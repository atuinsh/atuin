table! {
    history (id) {
        id -> Text,
        user -> Text,
        mac -> Text,
        timestamp -> Timestamp,
        data -> Text,
        signature -> Text,
    }
}

table! {
    users (id) {
        id -> Int8,
        email -> Text,
        api -> Text,
        key -> Text,
    }
}

allow_tables_to_appear_in_same_query!(history, users,);
