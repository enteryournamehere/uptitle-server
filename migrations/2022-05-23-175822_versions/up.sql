-- Your SQL goes here
CREATE TABLE IF NOT EXISTS "snapshot" (
	"project"	INTEGER NOT NULL,
	"timestamp"	BIGINT NOT NULL,
	"name" TEXT,
	"subtitles"	TEXT NOT NULL,
    PRIMARY KEY("project", "timestamp"),
	FOREIGN KEY("project") REFERENCES "project"("id")
);
CREATE UNIQUE INDEX IF NOT EXISTS "snapshot_index" ON "snapshot" (
	"project",
	"timestamp"
);
