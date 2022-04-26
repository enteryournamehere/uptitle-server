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

#[derive(Queryable, Serialize)]
#[serde(crate = "rocket::serde")]
pub struct Workspace {
    pub id: i32,
    pub name: String,
    pub owner: i32,
    pub shared: i32,
}

#[derive(Queryable, Serialize)]
#[serde(crate = "rocket::serde")]
pub struct Video {
    pub id: i32,
    pub source: String,
    pub identifier: String,
    pub duration: Option<i32>,
    pub waveform: Option<Vec<u8>>,
}

#[derive(Queryable, Serialize)]
#[serde(crate = "rocket::serde")]
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
