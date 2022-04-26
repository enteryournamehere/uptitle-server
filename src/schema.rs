// @generated automatically by Diesel CLI.

diesel::table! {
    project (id) {
        id -> Integer,
        workspace -> Integer,
        name -> Text,
        video -> Nullable<Integer>,
    }
}

diesel::table! {
    subtitle (id) {
        id -> Integer,
        project -> Integer,
        start -> Integer,
        end -> Integer,
        text -> Integer,
    }
}

diesel::table! {
    user (id) {
        id -> Integer,
        username -> Text,
        password -> Text,
        email -> Nullable<Text>,
        display_name -> Nullable<Text>,
    }
}

diesel::table! {
    video (id) {
        id -> Integer,
        source -> Text,
        identifier -> Text,
        duration -> Nullable<Integer>,
        waveform -> Nullable<Binary>,
    }
}

diesel::table! {
    workspace (id) {
        id -> Integer,
        name -> Text,
        owner -> Integer,
        shared -> Integer,
    }
}

diesel::table! {
    workspace_member (workspace, user) {
        workspace -> Integer,
        user -> Integer,
        role -> Integer,
    }
}

diesel::joinable!(project -> video (video));
diesel::joinable!(project -> workspace (workspace));
diesel::joinable!(subtitle -> project (project));
diesel::joinable!(workspace -> user (owner));
diesel::joinable!(workspace_member -> user (user));
diesel::joinable!(workspace_member -> workspace (workspace));

diesel::allow_tables_to_appear_in_same_query!(
    project,
    subtitle,
    user,
    video,
    workspace,
    workspace_member,
);
