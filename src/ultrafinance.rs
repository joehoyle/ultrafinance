use crate::models::*;
use crate::schema;
use anyhow::anyhow;
use chrono::Duration;
use diesel::*;
use sqlx::Row;

pub type DbPool =
    diesel::r2d2::Pool<diesel::r2d2::ConnectionManager<diesel::mysql::MysqlConnection>>;
pub type DbConnection =
    diesel::r2d2::PooledConnection<diesel::r2d2::ConnectionManager<diesel::mysql::MysqlConnection>>;

use std::collections::HashMap;
use std::env;
use std::sync::mpsc::channel;

pub fn is_dev() -> bool {
    env::var("IS_DEVELOPMENT")
        .map(|v| v.eq("1"))
        .unwrap_or(false)
}
pub fn import_transactions(
    account: &Account,
    con: &mut DbConnection,
) -> anyhow::Result<Vec<Transaction>> {
    use crate::schema::transactions::dsl::*;

    let latest_transaction: Option<Transaction> = transactions
        .filter(account_id.eq(account.id))
        .order(booking_date.desc())
        .limit(1)
        .first(con)
        .optional()
        .map_err(anyhow::Error::msg)?;
    // Get all transactions from 7 days before the last transaction to account for slow to confirm transaction.
    let from_date = latest_transaction.map(|t| t.booking_date - Duration::days(7));

    let other_transactions = account.source()?.transactions(&from_date, &None)?;

    let mut new_transactions: Vec<transaction::NewTransaction> = vec![];
    for transaction in other_transactions {
        let exists: bool = diesel::dsl::select(diesel::dsl::exists(
            schema::transactions::dsl::transactions
                .filter(external_id.eq(&transaction.id))
                .filter(account_id.eq(account.id)),
        ))
        .get_result(con)?;

        if exists {
            continue;
        }

        let mut new_transaction = NewTransaction::from(transaction);
        new_transaction.account_id = account.id;
        new_transaction.user_id = account.user_id;
        new_transactions.push(new_transaction);
    }

    let inserted = diesel::insert_into(transactions)
        .values(&new_transactions)
        .execute(con)?;

    // Hack to get all the transactions inserted
    let inserted_transactions: Vec<Transaction> = transactions
        .filter(account_id.eq(account.id))
        .order(id.desc())
        .limit(inserted as i64)
        .get_results(con)
        .map_err(anyhow::Error::msg)?;

    // Enrich the transactions that were inserted
    let inserted_transactions = enrich_transactions(inserted_transactions, con)?;

    // Create the triggers
    for transaction in &inserted_transactions {
        create_transaction_trigger_queue(transaction, con)?;
    }
    Ok(inserted_transactions)
}

pub async fn sqlx_import_transactions(
    account: &Account,
    db: &sqlx::MySqlPool,
) -> anyhow::Result<Vec<Transaction>> {
    let latest_transaction = sqlx::query_as!(
        Transaction,
        "SELECT * FROM transactions WHERE account_id = ? ORDER BY booking_date DESC LIMIT 1",
        account.id
    )
    .fetch_one(db)
    .await;

    // Get all transactions from 7 days before the last transaction to account for slow to confirm transaction.
    let from_date = latest_transaction
        .map(|t| t.booking_date - Duration::days(7))
        .ok();

    let other_transactions = account.source()?.transactions(&from_date, &None)?;

    let mut new_transactions: Vec<transaction::NewTransaction> = vec![];
    for transaction in other_transactions {
        let exists = sqlx::query(
            "SELECT EXISTS(SELECT 1 FROM transactions WHERE external_id = ? AND account_id = ?)",
        )
        .bind(&transaction.id)
        .bind(&account.id)
        .fetch_one(db)
        .await?;

        if exists.is_empty() {
            continue;
        }

        let mut new_transaction = NewTransaction::from(transaction);
        new_transaction.account_id = account.id;
        new_transaction.user_id = account.user_id;
        new_transactions.push(new_transaction);
    }

    let mut qb = sqlx::QueryBuilder::new("INSERT INTO transactions (external_id, creditor_name, debtor_name, remittance_information, booking_date, booking_datetime, transaction_amount, transaction_amount_currency, proprietary_bank_transaction_code, currency_exchange_rate, currency_exchange_source_currency, currency_exchange_target_currency, account_id, user_id)");
    qb.push_values(new_transactions, |mut b, t| {
        b.push_bind(t.external_id);
        b.push_bind(t.creditor_name);
        b.push_bind(t.debtor_name);
        b.push_bind(t.remittance_information);
        b.push_bind(t.booking_date);
        b.push_bind(t.booking_datetime);
        b.push_bind(t.transaction_amount);
        b.push_bind(t.transaction_amount_currency);
        b.push_bind(t.proprietary_bank_transaction_code);
        b.push_bind(t.currency_exchange_rate);
        b.push_bind(t.currency_exchange_source_currency);
        b.push_bind(t.currency_exchange_target_currency);
        b.push_bind(t.account_id);
        b.push_bind(t.user_id);
    });

    let insert = qb.build().execute(db).await?;

    // Hack to get all the transactions inserted
    let inserted_transactions: Vec<Transaction> = sqlx::query_as!(
        Transaction,
        "SELECT * FROM transactions WHERE account_id = ? ORDER BY id DESC LIMIT ?",
        account.id,
        insert.rows_affected()
    )
    .fetch_all(db)
    .await?;

    // Enrich the transactions that were inserted
    let inserted_transactions = sqlx_enrich_transactions(inserted_transactions, db).await?;

    // Create the triggers
    for transaction in &inserted_transactions {
        sqlx_create_transaction_trigger_queue(transaction, db).await?;
    }
    Ok(inserted_transactions)
}

pub fn create_transaction_trigger_queue(
    transaction: &Transaction,
    con: &mut DbConnection,
) -> anyhow::Result<()> {
    use schema::triggers::dsl::*;
    let transaction_triggers: Vec<Trigger> = Trigger::by_user(transaction.user_id)
        .filter(event.eq("transaction_created"))
        .load(con)?;

    let transaction_triggers = transaction_triggers
        .into_iter()
        .filter(|trigger| trigger.filter.matches(transaction))
        .collect::<Vec<Trigger>>();

    for transaction_trigger in &transaction_triggers {
        NewTriggerQueue {
            payload: serde_json::to_string(transaction)?,
            user_id: transaction.user_id,
            trigger_id: transaction_trigger.id,
        }
        .create(con)?;
    }

    Ok(())
}

pub async fn sqlx_create_transaction_trigger_queue(
    transaction: &Transaction,
    db: &sqlx::MySqlPool,
) -> anyhow::Result<()> {
    let transaction_triggers: Vec<Trigger> =
        Trigger::sqlx_for_user_for_event(transaction.user_id, "event", db).await?;

    let transaction_triggers = transaction_triggers
        .into_iter()
        .filter(|trigger| trigger.filter.matches(transaction))
        .collect::<Vec<Trigger>>();

    for transaction_trigger in &transaction_triggers {
        NewTriggerQueue {
            payload: serde_json::to_string(transaction)?,
            user_id: transaction.user_id,
            trigger_id: transaction_trigger.id,
        }
        .sqlx_create(db)
        .await?;
    }

    Ok(())
}

pub fn hash_api_key(api_key: &str) -> String {
    let salt = env::var("API_KEY_SALT").unwrap();
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let api_key = format!("{}:{}", salt, api_key);
    hasher.update(api_key);
    let hash = hasher.finalize();
    let hash = format!("{:x}", hash);

    hash
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    use rand::Rng;
    let salt: [u8; 32] = rand::thread_rng().gen();
    let config = argon2::Config::default();
    argon2::hash_encoded(password.as_bytes(), &salt, &config).map_err(|e| anyhow::anyhow!(e))
}

pub fn verify_password(hash: &str, password: &str) -> anyhow::Result<()> {
    let verified =
        argon2::verify_encoded(hash, password.as_bytes()).map_err(|e| anyhow::anyhow!(e))?;
    match verified {
        true => Ok(()),
        false => Err(anyhow!("Password incorrect.")),
    }
}

pub fn sync_accounts(
    accounts: &Vec<Account>,
    db_pool: &diesel::r2d2::Pool<diesel::r2d2::ConnectionManager<diesel::mysql::MysqlConnection>>,
) -> HashMap<u32, anyhow::Result<Vec<Transaction>>> {
    let thread_pool = threadpool::ThreadPool::new(8);
    let (tx, rx) = channel::<(u32, anyhow::Result<Vec<Transaction>>)>();

    for account in accounts {
        let tx = tx.clone();
        let db = db_pool.clone();
        let account = account.clone();
        #[allow(unused_must_use)]
        thread_pool.execute(move || {
            let mut con = db.get().unwrap();
            let result = import_transactions(&account, &mut con);
            tx.send((account.id, result));
        });
    }

    thread_pool.join();

    let mut result_map = HashMap::new();

    for (account_id, result) in rx.iter().take(accounts.len()) {
        result_map.insert(account_id, result);
    }

    result_map
}

pub fn process_trigger_queue(
    queue: &Vec<TriggerQueue>,
    db_pool: diesel::r2d2::Pool<diesel::r2d2::ConnectionManager<diesel::mysql::MysqlConnection>>,
) -> HashMap<u32, anyhow::Result<TriggerLog>> {
    let thread_pool = threadpool::ThreadPool::new(8);
    let (tx, rx) = channel::<(u32, anyhow::Result<TriggerLog>)>();

    for q in queue {
        let tx = tx.clone();
        let db = db_pool.clone();
        let q = q.clone();
        #[allow(unused_must_use)]
        thread_pool.execute(move || {
            let mut con = db.get().unwrap();
            let log = q.run(&mut con);
            tx.send((q.id, log));
        });
    }

    thread_pool.join();

    let mut result_map = HashMap::new();

    for (queue_id, log) in rx.iter().take(queue.len()) {
        result_map.insert(queue_id, log);
    }

    result_map
}

fn enrich_transactions(
    transactions: Vec<Transaction>,
    con: &mut DbConnection,
) -> anyhow::Result<Vec<Transaction>> {
    let client = crate::ntropy::ApiClient::new(env::var("NTROPY_API_KEY").unwrap());
    let enriched_transactions =
        client.enrich_transactions(transactions.into_iter().map(|t| t.into()).collect())?;
    let mut transactions: Vec<Transaction> = vec![];
    for enriched_transaction in enriched_transactions {
        match NewMerchant::try_from(&enriched_transaction) {
            Ok(merchant) => match merchant.create_or_fetch(con) {
                Ok(merchant) => {
                    let t_id = enriched_transaction.transaction_id.parse::<u32>().unwrap();
                    let mut transaction: Transaction = Transaction::by_id_only(t_id).first(con)?;
                    transaction.merchant_id = Some(merchant.id);
                    transaction.update(con)?;
                    transactions.push(transaction);
                }
                Err(err) => {
                    println!("Error creating merchant: {}", err);
                    continue;
                }
            },
            Err(err) => println!("Error getting merchant: {}", err),
        }
    }

    Ok(transactions)
}

async fn sqlx_enrich_transactions(
    transactions: Vec<Transaction>,
    db: &sqlx::MySqlPool,
) -> anyhow::Result<Vec<Transaction>> {
    let client = crate::ntropy::ApiClient::new(env::var("NTROPY_API_KEY").unwrap());
    let enriched_transactions = client
        .async_enrich_transactions(transactions.into_iter().map(|t| t.into()).collect())
        .await?;
    let mut transactions: Vec<Transaction> = vec![];
    for enriched_transaction in enriched_transactions {
        match NewMerchant::try_from(&enriched_transaction) {
            Ok(merchant) => match merchant.sqlx_create_or_fetch(db).await {
                Ok(merchant) => {
                    let t_id = enriched_transaction.transaction_id.parse::<u32>().unwrap();
                    let mut transaction: Transaction = Transaction::sqlx_by_id(t_id, db).await?;
                    transaction.merchant_id = Some(merchant.id);
                    transaction.sqlx_update(db).await?;
                    transactions.push(transaction);
                }
                Err(err) => {
                    println!("Error creating merchant: {}", err);
                    continue;
                }
            },
            Err(err) => println!("Error getting merchant: {}", err),
        }
    }

    Ok(transactions)
}
