diesel::table! {
    users (id) {
        id -> Int4,
        name -> Text,
        email -> Text,
        password -> Text,
        created_at -> Datetime,
        updated_at -> Datetime,
    }
}

diesel::table! {
    accounts (id) {
        id -> Int4,
        name -> Text,
        account_type -> Text,
        currency -> Text,
        product -> Nullable<Text>,
        cash_account_type -> Nullable<Text>,
        status -> Text,
        details -> Text,
        balance -> Float,
        owner_name -> Nullable<Text>,
        icon -> Nullable<Text>,
        institution_name -> Text,
        nordigen_id -> Text,
        user_id -> Int4,
        created_at -> Datetime,
        updated_at -> Datetime,
        config -> Nullable<Text>,
        number -> Nullable<Text>,
    }
}

diesel::table! {
    functions (id) {
        id -> Int4,
        name -> Text,
        function_type -> Text,
        source -> Text,
        user_id -> Int4,
        created_at -> Datetime,
        updated_at -> Datetime,
    }
}

diesel::table! {
    nordigen_requisitions (id) {
        id -> Int4,
        nordigen_id -> Text,
        status -> Text,
        user_id -> Int4,
        created_at -> Datetime,
        updated_at -> Datetime,
    }
}

diesel::table! {
    transactions (id) {
        id -> Int4,
        external_id -> Text,
        creditor_name -> Nullable<Text>,
        debtor_name -> Nullable<Text>,
        remittance_information -> Nullable<Text>,
        booking_date -> Date,
        booking_datetime -> Nullable<Datetime>,
        transaction_amount -> Text,
        transaction_amount_currency -> Text,
        proprietary_bank_transaction_code -> Nullable<Text>,
        currency_exchange_rate -> Nullable<Text>,
        currency_exchange_source_currency -> Nullable<Text>,
        currency_exchange_target_currency -> Nullable<Text>,
        merchant_id -> Nullable<Int4>,
        account_id -> Int4,
        user_id -> Int4,
        created_at -> Datetime,
        updated_at -> Datetime,
    }
}

diesel::table! {
    triggers (id) {
        id -> Int4,
        event -> Text,
        name -> Text,
        filter -> Text,
        params -> Text,
        user_id -> Int4,
        function_id -> Int4,
        created_at -> Datetime,
        updated_at -> Datetime,
    }
}

diesel::table! {
    trigger_log (id) {
        id -> Int4,
        payload -> Text,
        status -> Text,
        user_id -> Int4,
        trigger_id -> Int4,
        created_at -> Datetime,
        updated_at -> Datetime,
    }
}

diesel::table! {
    trigger_queue (id) {
        id -> Int4,
        payload -> Text,
        user_id -> Int4,
        trigger_id -> Int4,
        created_at -> Datetime,
        updated_at -> Datetime,
    }
}

diesel::table! {
    user_api_keys (api_key) {
        api_key -> Text,
        user_id -> Int4,
        created_at -> Datetime,
    }
}

diesel::table! {
    merchants (id) {
        id -> Int4,
        name -> Text,
        logo_url -> Nullable<Text>,
        location -> Nullable<Text>,
        location_structured -> Nullable<Text>,
        labels -> Nullable<Text>,
        external_id -> Nullable<Text>,
        website -> Nullable<Text>,
        created_at -> Datetime,
    }
}

diesel::joinable!(transactions -> merchants (merchant_id));
diesel::allow_tables_to_appear_in_same_query!(
    transactions,
    merchants,
);
diesel::sql_function!(fn last_insert_id() -> Integer);
