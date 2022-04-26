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

#[get("/workspace/list")]
async fn list_workspaces(db: DbConn) -> Result<Json<Vec<Workspace>>> {
    let ids: Vec<Workspace> = db.run(move |conn| workspace::table.load(conn)).await?;

    Ok(Json(ids))
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
async fn login(cookies: &CookieJar<'_>, info: Json<LoginInfo>, db: DbConn) -> Result<Json<User>> {
    let supplied_info = info.into_inner();
    let user: User = db
        .run(move |conn| {
            user::table
                .filter(user::username.eq(supplied_info.user))
                .first(conn)
        })
        .await?;

    let parsed_hash = PasswordHash::new(&user.password).expect("could not parse hash");
    match Argon2::default().verify_password(supplied_info.password.as_bytes(), &parsed_hash) {
        Ok(_) => {
            cookies.add_private(Cookie::new("auth", user.id.to_string()));
            Ok(Json(user))
        }
        Err(_) => Err(Debug(diesel::result::Error::NotFound)),
    }
}

// TODO use the returning thing https://github.com/diesel-rs/diesel/discussions/2684
// might need a newer diesel version but that caused errors in the model
no_arg_sql_function!(
    last_insert_rowid,
    diesel::sql_types::Integer,
    "Represents the SQL last_insert_row() function"
);

#[post("/register", data = "<info>")]
async fn register(
    cookies: &CookieJar<'_>,
    info: Json<LoginInfo>,
    db: DbConn,
) -> Result<String, String> {
    let supplied_info = info.into_inner();

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = match argon2.hash_password(supplied_info.password.as_bytes(), &salt) {
        Ok(hash) => hash,
        Err(_) => return Err("Could not generate hash".to_string()),
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
            Ok("Success".to_string())
        }
        Err(_) => Err("Could not create user".to_string()),
    }
}

/// Get user ID
#[get("/auth")]
fn auth(cookies: &CookieJar<'_>) -> Option<String> {
    cookies
        .get_private("auth")
        .map(|crumb| format!("User ID: {}", crumb.value()))
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
}
