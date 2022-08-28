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

use rocket::response::stream::{Event, EventStream};
use rocket::response::Debug;
use rocket::serde::{json::Json, Deserialize, Serialize};
use rocket::tokio::select;
use rocket::tokio::sync::broadcast::{channel, error::RecvError, Sender};
use rocket::{http::Status, Shutdown, State};
use rocket::{
    http::{Cookie, CookieJar},
    tokio::task,
};
use rocket::{
    request::{self, FromRequest, Outcome, Request},
    tokio::time::Instant,
};
use rocket_sync_db_pools::diesel;
use std::time::{SystemTime, UNIX_EPOCH};

use std::env;
use std::process::{Command, Stdio};

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
        name: project.name,
        source: video.source.clone(),
        video: Some(VideoInfo {
            id: video.identifier.clone(),
            duration: video.duration.unwrap_or(0),
        }),
        thumbnail: format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", video.identifier),
        duration: video.duration.unwrap_or(0),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(crate = "rocket::serde")]
struct ProjectCreationInfo {
    name: String,
    workspace: i32,
    video: String,
}

#[post("/project/create", data = "<project>")]
async fn create_project(
    project: Json<ProjectCreationInfo>,
    user: User,
    db: DbConn,
    queue: &State<Sender<SubtitleEvent>>,
) -> Result<String, Status> {
    let workspace_id = project.workspace;
    let user_is_in_workspace = db
        .run(move |conn| {
            workspace_member::table
                .filter(workspace_member::user.eq(user.id))
                .filter(workspace_member::workspace.eq(workspace_id))
                .first::<WorkspaceMember>(conn)
        })
        .await
        .is_ok();

    if !user_is_in_workspace {
        return Err(Status::Forbidden);
    }

    let project = project.into_inner();

    match youtube_video_exists(&project.video).await {
        Ok(false) | Err(_) => {
            return Err(Status::NotFound);
        }
        _ => {}
    }

    let new_video = NewVideo {
        identifier: project.video.clone(),
        source: "youtube".to_string(),
        duration: None,
        waveform: None,
    };
    let video_id = db
        .run(move |conn| {
            let result = diesel::insert_into(video::table)
                .values(new_video)
                .execute(conn);

            result?;

            diesel::select(last_insert_rowid).get_result::<i32>(conn)
        })
        .await
        .map_err(|_| Status::InternalServerError)?;

    let new_project = NewProject {
        name: project.name.clone(),
        workspace: project.workspace,
        video: Some(video_id),
    };

    let project_id = db
        .run(move |conn| {
            let result = diesel::insert_into(project::table)
                .values(new_project)
                .execute(conn);

            result?;

            diesel::select(last_insert_rowid).get_result::<i32>(conn)
        })
        .await
        .map_err(|_| Status::InternalServerError)?;

    let sender = queue.inner().to_owned();
    let youtube_id = project.video.clone();
    task::spawn(async move {
        if let Ok(()) = download_youtube_audio(db, youtube_id.as_str()).await {
            let _ = sender.send(SubtitleEvent {
                info: SubtitleEventType::WaveformReady,
                project: project_id,
            });
        }
    });

    Ok(project_id.to_string())
}

#[derive(Deserialize, Debug)]
#[serde(crate = "rocket::serde")]
#[serde(rename_all = "camelCase")]
struct YoutubeResponseTest {
    page_info: PageInfo,
}

#[derive(Deserialize, Debug)]
#[serde(crate = "rocket::serde")]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    total_results: u32,
    #[allow(dead_code)]
    results_per_page: u32,
}

async fn youtube_video_exists(youtube_id: &str) -> core::result::Result<bool, reqwest::Error> {
    let url = format!(
        "https://www.googleapis.com/youtube/v3/videos?part=id&id={}&key={}",
        youtube_id,
        env::var("YOUTUBE_API_KEY").unwrap()
    );

    let response = reqwest::get(&url)
        .await?
        .json::<YoutubeResponseTest>()
        .await?;

    Ok(response.page_info.total_results > 0)
}

// Status as the error type is pretty much useless, I'll probably change it later
async fn download_youtube_audio(db: DbConn, youtube_id: &str) -> Result<(), Status> {
    let start = Instant::now();

    let client = ytextract::Client::new();

    // Find available streams
    let stream = client
        .streams(
            youtube_id
                .parse()
                .map_err(|_| Status::InternalServerError)?,
        )
        .await
        .map_err(|_| Status::InternalServerError)?
        // Filter to audio-only and find one with sample rate 48000...
        // 44100 results in waveform alignment issues (makes for 400.9 px per second)
        .filter(|stream| match stream {
            ytextract::Stream::Audio(audio) => audio.sample_rate() == 48000,
            _ => false,
        })
        // Get the one with the lowest .bitrate()
        .min_by(|a, b| a.bitrate().cmp(&b.bitrate()))
        .ok_or(Status::InternalServerError)?;

    let duration_ms = stream
        .duration()
        .expect("Stream has no duration")
        .as_millis() as i32;

    let waveform_filename = format!("/tmp/uptitle-{}.dat", youtube_id);

    // Download and convert audio in background
    let stream_url = stream.url().to_string();
    let waveform_filename2 = waveform_filename.to_owned();

    let waveform = task::spawn_blocking(move || {
        let waveformer = Command::new("audiowaveform")
            .args([
                "--input-format",
                "wav",
                "-o",
                &waveform_filename2,
                "-b",
                "8",
                "--pixels-per-second",
                "400",
            ])
            .stdin(Stdio::piped())
            .spawn()
            .expect("audiowaveform failed");

        Command::new("ffmpeg")
            .args(["-i", &stream_url, "-f", "wav", "-"])
            .stdout(waveformer.stdin.unwrap())
            .output()
            .expect("ffmpeg failed");

        std::fs::read(&waveform_filename2)
    })
    .await
    .map_err(|_| Status::InternalServerError)?
    .map_err(|_| Status::InternalServerError)?;

    // Update database
    let youtube_id = youtube_id.to_owned();
    db.run(move |conn| {
        diesel::update(video::table)
            .filter(video::identifier.eq(youtube_id))
            .set((
                video::waveform.eq(Some(waveform)),
                video::duration.eq(Some(duration_ms)),
            ))
            .execute(conn)
    })
    .await
    .map_err(|_| Status::InternalServerError)?;

    // Clean up
    std::fs::remove_file(&waveform_filename).expect("could not delete file");

    println!("done in {:?}", start.elapsed());

    Ok(())
}

#[get("/waveform/<youtube_id>")]
async fn get_waveform(youtube_id: String, db: DbConn) -> Result<Vec<u8>, Status> {
    let waveform = db
        .run(move |conn| {
            video::table
                .filter(video::identifier.eq(youtube_id))
                .select(video::waveform)
                .first::<Option<Vec<u8>>>(conn)
        })
        .await
        .map_err(|_| Status::InternalServerError)?;

    match waveform {
        Some(waveform) => Ok(waveform),
        None => Err(Status::NotFound),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(crate = "rocket::serde")]
struct CreateEventData {
    pub subtitle: i32,
    pub text: String,
    pub start: i32,
    pub end: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(crate = "rocket::serde")]
struct EditEventData {
    pub subtitle: i32,
    pub text: Option<String>,
    pub start: Option<i32>,
    pub end: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(crate = "rocket::serde")]
struct DeleteEventData {
    pub subtitle: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(crate = "rocket::serde")]
enum SubtitleEventType {
    WaveformReady,
    SubtitleCreate(CreateEventData),
    SubtitleEdit(EditEventData),
    SubtitleDelete(DeleteEventData),
}

#[derive(Debug, Clone, Serialize)]
#[serde(crate = "rocket::serde")]
struct SubtitleEvent {
    info: SubtitleEventType,
    project: i32,
}

#[get("/project/<project_id>/events")]
async fn events(
    project_id: i32,
    queue: &State<Sender<SubtitleEvent>>,
    mut end: Shutdown,
) -> EventStream![] {
    let mut rx = queue.subscribe();
    EventStream! {
        loop {
            let msg = select! {
                msg = rx.recv() => match msg {
                    Ok(msg) => msg,
                    Err(RecvError::Closed) => break,
                    Err(RecvError::Lagged(_)) => continue,
                },
                _ = &mut end => break,
            };

            if msg.project != project_id {
                continue;
            }

            yield Event::json(&msg).event(match msg.info {
                SubtitleEventType::WaveformReady => "waveform_ready",
                SubtitleEventType::SubtitleEdit(_) => "subtitle_edit",
                SubtitleEventType::SubtitleCreate(_) => "subtitle_create",
                SubtitleEventType::SubtitleDelete(_) => "subtitle_delete",
            });
        }
    }
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
        .run(move |conn| {
            Subtitle::belonging_to(&project)
                .order(subtitle::start.asc())
                .load::<Subtitle>(conn)
        })
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
    queue: &State<Sender<SubtitleEvent>>,
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
    let subtitle_clone = subtitle.clone();

    let new_id: i32 = db
        .run(move |conn| {
            let result = diesel::insert_into(schema::subtitle::table)
                .values(&subtitle_clone)
                .execute(conn);

            result?;

            diesel::select(last_insert_rowid).get_result::<i32>(conn)
        })
        .await
        .map_err(|_| Status::InternalServerError)?;

    // Broadcast SSE
    let _ = queue.send(SubtitleEvent {
        info: SubtitleEventType::SubtitleCreate(CreateEventData {
            subtitle: new_id,
            start: subtitle.start,
            end: subtitle.end,
            text: subtitle.text,
        }),
        project: project.id,
    });

    Ok(new_id.to_string())
}

#[delete("/project/<project_id>/subtitle/<subtitle_id>")]
async fn delete_subtitle(
    project_id: i32,
    subtitle_id: i32,
    user: User,
    db: DbConn,
    queue: &State<Sender<SubtitleEvent>>,
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

    // Broadcast SSE
    let _ = queue.send(SubtitleEvent {
        info: SubtitleEventType::SubtitleDelete(DeleteEventData {
            subtitle: subtitle_id,
        }),
        project: project.id,
    });

    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(crate = "rocket::serde")]
struct SubtitleEditInfo {
    start: Option<i32>,
    end: Option<i32>,
    text: Option<String>,
}

#[patch("/project/<project_id>/subtitle/<subtitle_id>", data = "<info>")]
async fn edit_subtitle(
    project_id: i32,
    subtitle_id: i32,
    info: Json<SubtitleEditInfo>,
    user: User,
    db: DbConn,
    queue: &State<Sender<SubtitleEvent>>,
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

    let mut subtitle: Subtitle = db
        .run(move |conn| {
            subtitle::table
                .filter(subtitle::id.eq(subtitle_id))
                .filter(subtitle::project.eq(project.id))
                .first::<Subtitle>(conn)
        })
        .await
        .map_err(|_| Status::NotFound)?;

    if let Some(start) = info.start {
        subtitle.start = start;
    }
    if let Some(end) = info.end {
        subtitle.end = end;
    }
    if let Some(text) = info.text.clone() {
        subtitle.text = text;
    }

    let updated_count: usize = db
        .run(move |conn| {
            diesel::update(subtitle::table)
                .filter(subtitle::id.eq(subtitle_id))
                .filter(subtitle::project.eq(project.id))
                .set((
                    subtitle::start.eq(subtitle.start),
                    subtitle::end.eq(subtitle.end),
                    subtitle::text.eq(subtitle.text),
                ))
                .execute(conn)
        })
        .await
        .map_err(|_| Status::InternalServerError)?;

    if updated_count == 0 {
        // This probably shouldn't happen because we're already fetching the subtitle earlier
        return Err(Status::NotFound);
    }

    // Broadcast SSE
    let _ = queue.send(SubtitleEvent {
        info: SubtitleEventType::SubtitleEdit(EditEventData {
            subtitle: subtitle_id,
            start: info.start,
            end: info.end,
            text: info.text.clone(),
        }),
        project: project.id,
    });

    Ok(())
}

#[post("/project/<project_id>/snapshot/create")]
async fn create_snapshot(project_id: i32, user: User, db: DbConn) -> Result<String, Status> {
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

    // Get all subtitles for this project
    let subtitles: Vec<Subtitle> = db
        .run(move |conn| {
            subtitle::table
                .filter(subtitle::project.eq(project.id))
                .order(subtitle::start)
                .load::<Subtitle>(conn)
        })
        .await
        .map_err(|_| Status::InternalServerError)?;

    // Put them into a big json array
    let subtitles_json = rocket::serde::json::serde_json::to_string(&subtitles)
        .map_err(|_| Status::InternalServerError)?;

    let _affected_rows: usize = db
        .run(move |conn| {
            diesel::insert_into(schema::snapshot::table)
                .values(&Snapshot {
                    project: project.id,
                    name: None,
                    timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("we're in the past")
                        .as_secs() as i64,
                    subtitles: subtitles_json,
                })
                .execute(conn)
        })
        .await
        .map_err(|_| Status::InternalServerError)?;
    Ok("done".to_string())
}

#[derive(Debug, Serialize)]
#[serde(crate = "rocket::serde")]
struct SnapshotInfo {
    project: i32,
    timestamp: i64,
    name: Option<String>,
}

#[get("/project/<project_id>/snapshot/list", rank = 1)]
async fn list_snapshots(
    project_id: i32,
    user: User,
    db: DbConn,
) -> Result<Json<Vec<SnapshotInfo>>, Status> {
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

    let snapshots: Vec<SnapshotInfo> = db
        .run(move |conn| {
            schema::snapshot::table
                .filter(schema::snapshot::project.eq(project.id))
                .order(schema::snapshot::timestamp.desc())
                .select((
                    schema::snapshot::project,
                    schema::snapshot::timestamp,
                    schema::snapshot::name,
                ))
                .load::<(i32, i64, Option<String>)>(conn)
        })
        .await
        .map_err(|_| Status::InternalServerError)?
        .into_iter()
        .map(|(project, timestamp, name)| SnapshotInfo {
            project,
            timestamp,
            name,
        })
        .collect();

    Ok(Json(snapshots))
}

#[derive(Serialize)]
#[serde(crate = "rocket::serde")]
struct SnapshotResponse {
    name: Option<String>,
    subtitles: Vec<Subtitle>,
}

#[get("/project/<project_id>/snapshot/<timestamp>", rank = 2)]
async fn get_snapshot(
    project_id: i32,
    timestamp: i64,
    user: User,
    db: DbConn,
) -> Result<Json<SnapshotResponse>, Status> {
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

    let snapshot: Snapshot = db
        .run(move |conn| {
            schema::snapshot::table
                .filter(schema::snapshot::project.eq(project.id))
                .filter(schema::snapshot::timestamp.eq(timestamp))
                .first::<Snapshot>(conn)
        })
        .await
        .map_err(|_| Status::NotFound)?;

    // This is kinda unnecessary because it first deserializes the json and then re-serializes it.
    // But this way the return type of this function is the clearest.
    let subtitles: Vec<Subtitle> = rocket::serde::json::serde_json::from_str(&snapshot.subtitles)
        .map_err(|_| Status::InternalServerError)?;

    let response: SnapshotResponse = SnapshotResponse {
        name: snapshot.name,
        subtitles,
    };

    Ok(Json(response))
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
struct SnapshotPatchInfo {
    name: String,
}

#[patch("/project/<project_id>/snapshot/<timestamp>", data = "<info>")]
async fn edit_snapshot(
    project_id: i32,
    timestamp: i64,
    info: Json<SnapshotPatchInfo>,
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

    let _affected_rows: usize = db
        .run(move |conn| {
            diesel::update(schema::snapshot::table)
                .filter(schema::snapshot::project.eq(project.id))
                .filter(schema::snapshot::timestamp.eq(timestamp))
                .set(schema::snapshot::name.eq(info.name.clone()))
                .execute(conn)
        })
        .await
        .map_err(|_| Status::InternalServerError)?;
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

            result?;

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
    dotenv::dotenv().ok();

    rocket::build()
        .attach(DbConn::fairing())
        .manage(channel::<SubtitleEvent>(1024).0)
        .mount("/api", routes![secure]) // Temp
        .mount("/api", routes![login, auth, logout, register]) // Auth
        .mount("/api", routes![list_workspaces]) // Workspaces
        .mount(
            "/api",
            routes![get_project, create_project, events, get_waveform],
        ) // Projects
        .mount(
            "/api",
            routes![
                get_subtitle_list,
                create_subtitle,
                edit_subtitle,
                delete_subtitle
            ],
        ) // Subtitles
        .mount(
            "/api",
            routes![list_snapshots, create_snapshot, get_snapshot, edit_snapshot],
        ) // Snapshots
}
