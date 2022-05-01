use crate::schema::*;
use rocket::serde::Serialize;

#[derive(Queryable, Serialize)]
#[serde(crate = "rocket::serde")]
pub struct User {
    pub id: i32,
    pub username: String,
    pub password: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Insertable)]
#[table_name = "user"]
pub struct NewUser {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Queryable, Identifiable)]
#[serde(crate = "rocket::serde")]
#[table_name = "workspace"]
pub struct Workspace {
    pub id: i32,
    pub name: String,
    pub owner: i32,
    pub shared: i32,
}

#[derive(Debug, Clone, Serialize, Queryable, Identifiable, Associations)]
#[serde(crate = "rocket::serde")]
#[belongs_to(Workspace, foreign_key="workspace")]
#[table_name = "workspace_member"]
#[primary_key(workspace, user)]
pub struct WorkspaceMember {
    pub workspace: i32,
    pub user: i32,
    pub role: i32,
}

#[derive(Debug, Queryable, Serialize)]
#[serde(crate = "rocket::serde")]
pub struct Video {
    pub id: i32,
    pub source: String,
    pub identifier: String,
    pub duration: Option<i32>,
    pub waveform: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Queryable, Serialize, Identifiable, Associations)]
#[serde(crate = "rocket::serde")]
#[belongs_to(Workspace, foreign_key="workspace")]
#[table_name="project"]
#[primary_key(id)]
pub struct Project {
    pub id: i32,
    pub workspace: i32,
    pub name: String,
    pub video: Option<i32>,
}

#[derive(Queryable, Serialize)]
#[serde(crate = "rocket::serde")]
pub struct Subtitle {
    pub id: i32,
    pub project: i32,
    pub start: i32,
    pub end: i32,
    pub text: i32,
}
