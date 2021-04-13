table! {
    history (id) {
        id -> Int8,
        client_id -> Text,
        user_id -> Int8,
        hostname -> Text,
        timestamp -> Timestamp,
        data -> Varchar,
        created_at -> Timestamp,
    }
}

table! {
    sessions (id) {
        id -> Int8,
        user_id -> Int8,
        token -> Varchar,
    }
}

table! {
    users (id) {
        id -> Int8,
        username -> Varchar,
        email -> Varchar,
        password -> Varchar,
    }
}

allow_tables_to_appear_in_same_query!(history, sessions, users,);
