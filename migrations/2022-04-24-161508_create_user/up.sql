CREATE TABLE IF NOT EXISTS "project" (
	"id"	INTEGER NOT NULL UNIQUE,
	"workspace"	INTEGER NOT NULL,
	"name"	TEXT NOT NULL,
	"video"	INTEGER,
	FOREIGN KEY("workspace") REFERENCES "workspace"("id") ON DELETE CASCADE,
	PRIMARY KEY("id" AUTOINCREMENT),
	FOREIGN KEY("video") REFERENCES "video"("id")
);
CREATE TABLE IF NOT EXISTS "subtitle" (
	"id"	INTEGER NOT NULL UNIQUE,
	"project"	INTEGER NOT NULL,
	"start"	INTEGER NOT NULL,
	"end"	INTEGER NOT NULL,
	"text"	INTEGER NOT NULL,
	FOREIGN KEY("project") REFERENCES "project"("id") ON DELETE CASCADE,
	PRIMARY KEY("id" AUTOINCREMENT)
);
CREATE TABLE IF NOT EXISTS "user" (
	"id"	INTEGER NOT NULL UNIQUE,
	"username"	TEXT NOT NULL UNIQUE,
	"password"	TEXT NOT NULL,
	"email"	TEXT,
	"display_name"	TEXT,
	PRIMARY KEY("id" AUTOINCREMENT)
);
CREATE TABLE IF NOT EXISTS "video" (
	"id"	INTEGER NOT NULL UNIQUE,
	"source"	TEXT NOT NULL,
	"identifier"	TEXT NOT NULL,
	"duration"	INTEGER,
	"waveform"	BLOB,
	PRIMARY KEY("id" AUTOINCREMENT)
);
CREATE TABLE IF NOT EXISTS "workspace" (
	"id"	INTEGER NOT NULL UNIQUE,
	"name"	TEXT NOT NULL,
	"owner"	INTEGER NOT NULL,
	"shared"	INTEGER NOT NULL DEFAULT 0,
	FOREIGN KEY("owner") REFERENCES "user"("id"),
	PRIMARY KEY("id" AUTOINCREMENT)
);
CREATE TABLE IF NOT EXISTS "workspace_member" (
	"workspace"	INTEGER NOT NULL,
	"user"	INTEGER NOT NULL,
	"role"	INTEGER NOT NULL DEFAULT 0,
	FOREIGN KEY("user") REFERENCES "user"("id") ON DELETE CASCADE,
	FOREIGN KEY("workspace") REFERENCES "workspace"("id") ON DELETE CASCADE
	primary key ("workspace", "user")
);
CREATE UNIQUE INDEX IF NOT EXISTS "workspace_member_index" ON "workspace_member" (
	"workspace",
	"user"
);
