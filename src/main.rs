#[macro_use]
extern crate rocket;
#[macro_use]
extern crate rocket_sync_db_pools;
#[macro_use]
extern crate diesel as diesel2;

use self::diesel::prelude::*;

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

use rocket::http::Status;
use rocket::http::{Cookie, CookieJar};
use rocket::request::{self, FromRequest, Outcome, Request};
use rocket::response::Debug;
use rocket::serde::{json::Json, Deserialize, Serialize};
use rocket_sync_db_pools::diesel;

use self::diesel::sqlite::SqliteConnection;

pub mod models;
pub mod schema;

use crate::models::*;
use crate::schema::*;

#[database("diesel")]
pub struct DbConn(SqliteConnection);

type Result<T, E = Debug<diesel::result::Error>> = std::result::Result<T, E>;

// Workspace
#[derive(Serialize, Debug)]
#[serde(crate = "rocket::serde")]
struct WorkspaceInfo {
    id: i32,
    name: String,
    shared: i32,
    members: Vec<WorkspaceMemberInfo>,
    projects: Vec<ProjectInfo>,
}

#[derive(Serialize, Debug)]
#[serde(crate = "rocket::serde")]
struct WorkspaceMemberInfo {
    name: String,
    role: i32,
}
#[derive(Serialize, Debug)]
#[serde(crate = "rocket::serde")]
struct ProjectInfo {
    id: i32,
    workspace: i32,
    name: String,
    source: String,
    video: Option<VideoInfo>,
    thumbnail: String,
    duration: i32,
}

#[derive(Serialize, Debug)]
#[serde(crate = "rocket::serde")]
struct VideoInfo {
    id: String,
    duration: i32,
}

#[get("/workspace/list")]
async fn list_workspaces(db: DbConn, user: User) -> Result<Json<Vec<WorkspaceInfo>>> {
    let workspaces: Vec<Workspace> = db
        .run(move |conn| {
            workspace::table
                .inner_join(workspace_member::table)
                .filter(workspace_member::user.eq(user.id))
                .select(workspace::all_columns)
                .load::<Workspace>(conn)
        })
        .await?;

    let mut workspace_infos: Vec<WorkspaceInfo> = Vec::new();
    for workspace in workspaces {
        let workspace_clone = workspace.clone();
        let members: Vec<(WorkspaceMember, User)> = db
            .run(move |conn| {
                WorkspaceMember::belonging_to(&workspace_clone)
                    .inner_join(user::table)
                    .load::<(WorkspaceMember, User)>(conn)
            })
            .await?;
        let member_infos: Vec<WorkspaceMemberInfo> = members
            .iter()
            .map(|(member, user)| WorkspaceMemberInfo {
                name: user
                    .display_name
                    .as_ref()
                    .unwrap_or(&user.username)
                    .to_string(),
                role: member.role,
            })
            .collect();
        let workspace_clone2 = workspace.clone(); // is there a better way to do this?
        let projects: Vec<(Project, Video)> = db
            .run(move |conn| {
                Project::belonging_to(&workspace_clone2)
                    .inner_join(schema::video::table)
                    .load::<(Project, Video)>(conn)
            })
            .await?;

        let project_infos: Vec<ProjectInfo> = projects
            .iter()
            .map(|(project, video)| ProjectInfo {
                id: project.id,
                workspace: project.workspace,
                name: project.name.clone(),
                source: video.source.clone(),
                video: Some(VideoInfo {
                    id: video.identifier.clone(),
                    duration: video.duration.unwrap_or(0),
                }),
                thumbnail: format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", video.identifier),
                duration: video.duration.unwrap_or(0),
            })
            .collect();

        workspace_infos.push(WorkspaceInfo {
            id: workspace.id,
            name: workspace.name,
            shared: workspace.shared,
            members: member_infos,
            projects: project_infos,
        });
    }

    Ok(Json(workspace_infos))
}

#[get("/project/<id>")]
async fn get_project(id: i32, user: User, db: DbConn) -> Result<Json<ProjectInfo>, Status> {
    let (project, video): (Project, Video) = db
        .run(move |conn| {
            project::table
                .inner_join(workspace::table.left_join(workspace_member::table))
                .filter(workspace_member::user.eq(user.id))
                .filter(project::id.eq(id))
                .inner_join(schema::video::table)
                .select((project::all_columns, video::all_columns))
                .first::<(Project, Video)>(conn)
        })
        .await
        .map_err(|_| Status::NotFound)?;

    Ok(Json(ProjectInfo {
        id: project.id,
        workspace: project.workspace,
        name: project.name.clone(),
        source: video.source.clone(),
        video: Some(VideoInfo {
            id: video.identifier.clone(),
            duration: video.duration.unwrap_or(0),
        }),
        thumbnail: format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", video.identifier),
        duration: video.duration.unwrap_or(0),
    }))
}

#[get("/project/<id>/subtitle/list")]
async fn get_subtitle_list(id: i32, user: User, db: DbConn) -> Result<Json<Vec<Subtitle>>, Status> {
    let project: Project = db
        .run(move |conn| {
            project::table
                .inner_join(workspace::table.left_join(workspace_member::table))
                .filter(workspace_member::user.eq(user.id))
                .filter(project::id.eq(id))
                .select(project::all_columns)
                .first::<Project>(conn)
        })
        .await
        .map_err(|_| Status::NotFound)?;

    let subtitles: Vec<Subtitle> = db
        .run(move |conn| Subtitle::belonging_to(&project).load::<Subtitle>(conn))
        .await
        .map_err(|_| Status::InternalServerError)?;

    Ok(Json(subtitles))
}

#[derive(Debug, Deserialize)]
#[serde(crate = "rocket::serde")]
struct SubtitleCreationInfo {
    start: i32,
    end: i32,
    text: String,
}

#[post("/project/<id>/subtitle/create", data = "<info>")]
async fn create_subtitle(
    id: i32,
    user: User,
    info: Json<SubtitleCreationInfo>,
    db: DbConn,
) -> Result<String, Status> {
    let project: Project = db
        .run(move |conn| {
            project::table
                .inner_join(workspace::table.left_join(workspace_member::table))
                .filter(workspace_member::user.eq(user.id))
                .filter(project::id.eq(id))
                .select(project::all_columns)
                .first::<Project>(conn)
        })
        .await
        .map_err(|_| Status::NotFound)?;

    let subtitle = NewSubtitle {
        project: project.id,
        start: info.start,
        end: info.end,
        text: info.text.clone(),
    };

    let new_id: i32 = db
        .run(move |conn| {
            let result = diesel::insert_into(schema::subtitle::table)
                .values(&subtitle)
                .execute(conn);

            if let Err(message) = result {
                return Err(message);
            }

            diesel::select(last_insert_rowid).get_result::<i32>(conn)
        })
        .await
        .map_err(|_| Status::InternalServerError)?;

    Ok(new_id.to_string())
}

#[delete("/project/<project_id>/subtitle/<subtitle_id>")]
async fn delete_subtitle(
    project_id: i32,
    subtitle_id: i32,
    user: User,
    db: DbConn,
) -> Result<(), Status> {
    let project: Project = db
        .run(move |conn| {
            project::table
                .inner_join(workspace::table.left_join(workspace_member::table))
                .filter(workspace_member::user.eq(user.id))
                .filter(project::id.eq(project_id))
                .select(project::all_columns)
                .first::<Project>(conn)
        })
        .await
        .map_err(|_| Status::NotFound)?;

    let deleted_count: usize = db
        .run(move |conn| {
            diesel::delete(subtitle::table)
                .filter(subtitle::id.eq(subtitle_id))
                .filter(subtitle::project.eq(project.id))
                .execute(conn)
        })
        .await
        .map_err(|_| Status::InternalServerError)?;

    if deleted_count == 0 {
        return Err(Status::NotFound);
    }

    Ok(())
}

// Authentication

// Ensure user is logged in and get their info from the DB
#[rocket::async_trait]
impl<'r> FromRequest<'r> for User {
    type Error = &'static str;

    async fn from_request(request: &'r Request<'_>) -> request::Outcome<User, Self::Error> {
        let db = match request.guard::<DbConn>().await {
            Outcome::Success(c) => c,
            _ => {
                return Outcome::Failure((Status::ServiceUnavailable, "An internal error occured."))
            }
        };

        let auth_cookie = request.cookies().get_private("auth");
        if let Some(cookie) = auth_cookie {
            let user = db
                .run(move |conn| {
                    user::table
                        .filter(
                            user::id.eq(cookie
                                .value()
                                .parse::<i32>()
                                .expect("Auth cookie was not an int")),
                        )
                        .first(conn)
                })
                .await;
            match user {
                Ok(user) => Outcome::Success(user),
                Err(_) => Outcome::Failure((Status::Unauthorized, "User not found")),
            }
        } else {
            Outcome::Failure((Status::Unauthorized, "No cookies?"))
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(crate = "rocket::serde")]
struct LoginInfo {
    user: String,
    password: String,
}

#[post("/login", data = "<info>")]
async fn login(
    cookies: &CookieJar<'_>,
    info: Json<LoginInfo>,
    db: DbConn,
) -> Result<Json<GenericResponse>, (Status, &'static str)> {
    let supplied_info = info.into_inner();
    let user: User = db
        .run(move |conn| {
            user::table
                .filter(user::username.eq(supplied_info.user))
                .first(conn)
        })
        .await
        .map_err(|_| (Status::Unauthorized, "Username incorrect"))?;

    let parsed_hash = PasswordHash::new(&user.password)
        .map_err(|_| (Status::InternalServerError, "An internal error occured"))?;
    Argon2::default()
        .verify_password(supplied_info.password.as_bytes(), &parsed_hash)
        .map_err(|_| (Status::Unauthorized, "Password incorrect"))?;

    cookies.add_private(Cookie::new("auth", user.id.to_string()));
    Ok(Json(GenericResponse {
        error: false,
        message: Some("Logged in"),
    }))
}

// TODO use the returning thing https://github.com/diesel-rs/diesel/discussions/2684
// might need a newer diesel version but that caused errors in the model
no_arg_sql_function!(
    last_insert_rowid,
    diesel::sql_types::Integer,
    "Represents the SQL last_insert_row() function"
);

#[derive(Serialize)]
#[serde(crate = "rocket::serde")]
struct GenericResponse {
    error: bool,
    message: Option<&'static str>,
}

#[post("/register", data = "<info>")]
async fn register(
    cookies: &CookieJar<'_>,
    info: Json<LoginInfo>,
    db: DbConn,
) -> Result<Json<GenericResponse>, (Status, &'static str)> {
    let supplied_info = info.into_inner();
    // clone variable to move it into the closure, maybe this can be done in a nicer way?
    let cloned_name = supplied_info.user.clone();

    let existing_user = db
        .run(move |conn| {
            user::table
                .filter(user::username.eq(cloned_name.as_str()))
                .count()
                .get_result::<i64>(conn)
        })
        .await
        .map_err(|_| (Status::InternalServerError, "An internal error occured"))?;

    if existing_user > 0 {
        return Err((Status::BadRequest, "Username not available"));
    }

    if supplied_info.password.len() < 8 {
        return Err((Status::BadRequest, "Password must be at least 8 characters"));
    }

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = match argon2.hash_password(supplied_info.password.as_bytes(), &salt) {
        Ok(hash) => hash,
        Err(_) => return Err((Status::InternalServerError, "An internal error occured")),
    };
    let new_user = NewUser {
        username: supplied_info.user,
        password: password_hash.to_string(),
    };

    let user_id = db
        .run(move |conn| {
            let result = diesel::insert_into(user::table)
                .values(&new_user)
                .execute(conn);
            if let Err(message) = result {
                return Err(message);
            }

            diesel::select(last_insert_rowid).get_result::<i32>(conn)
        })
        .await;

    match user_id {
        Ok(id) => {
            cookies.add_private(Cookie::new("auth", id.to_string()));
            Ok(Json(GenericResponse {
                error: false,
                message: None,
            }))
        }
        Err(_) => Err((Status::InternalServerError, "An internal error occured")),
    }
}

/// Publicly visible stuff
#[derive(Serialize)]
#[serde(crate = "rocket::serde")]
struct UserInfo {
    name: String,
}

/// Get user ID
#[get("/auth")]
fn auth(user: User) -> Json<UserInfo> {
    let info = UserInfo {
        name: user.username,
    };
    Json(info)
}

/// Test thing
#[get("/secure")]
fn secure(user: User) -> Json<User> {
    Json(user)
}

/// Remove the auth cookie.
#[post("/logout")]
fn logout(cookies: &CookieJar<'_>) -> String {
    cookies.remove_private(Cookie::named("auth"));
    "Goodbye".into()
}

#[launch]
fn rocket() -> _ {
    rocket::build()
        .attach(DbConn::fairing())
        .mount("/api", routes![secure]) // Temp
        .mount("/api", routes![login, auth, logout, register]) // Auth
        .mount("/api", routes![list_workspaces]) // Workspaces
        .mount("/api", routes![get_project]) // Projects
        .mount(
            "/api",
            routes![get_subtitle_list, create_subtitle, delete_subtitle],
        ) // Subtitles
}
