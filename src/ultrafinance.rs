use crate::accounts::SourceAccount;
use crate::models::*;
use anyhow::anyhow;
use chrono::Duration;
use log::info;
use tokio::runtime::Runtime;

use std::collections::HashMap;
use std::{env, thread};

pub fn is_dev() -> bool {
    env::var("IS_DEVELOPMENT")
        .map(|v| v.eq("1"))
        .unwrap_or(false)
}

pub async fn sqlx_import_transactions(
    account: &Account,
    db: &sqlx::MySqlPool,
) -> anyhow::Result<Vec<Transaction>> {
    info!("Importing transactions for account: {}", account.id);
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

    info!("Date of latest transaction for account minus 1 week to account for pending: {:?}", from_date);

    let account_clone = account.clone();
    let other_transactions = account_clone.source()?.transactions(&from_date, &None).await?;

    info!("Found {} transactions for account: {}", other_transactions.len(), account.id);

    let mut new_transactions: Vec<transaction::NewTransaction> = vec![];
    for transaction in other_transactions {
        let exists = sqlx::query_scalar!(
            "SELECT EXISTS(SELECT 1 FROM transactions WHERE external_id = ? AND account_id = ?)", &transaction.id, &account.id)
        .fetch_one(db)
        .await?;
        if exists > 0 {
            info!("Transaction {} already exists", transaction.id);
            continue;
        }

        let mut new_transaction = NewTransaction::from(transaction);
        new_transaction.account_id = account.id;
        new_transaction.user_id = account.user_id;
        new_transactions.push(new_transaction);
    }

    if new_transactions.is_empty() {
        return Ok(vec![]);
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
    let query = qb.build();
    let insert = query.execute(db).await?;

    // Hack to get all the transactions inserted
    let inserted_transactions: Vec<Transaction> = sqlx::query_as!(
        Transaction,
        "SELECT * FROM transactions WHERE account_id = ? ORDER BY id DESC LIMIT ?",
        account.id,
        insert.rows_affected()
    )
    .fetch_all(db)
    .await?;

    info!("Sucessfully imported {} transactions for account: {}", inserted_transactions.len(), account.id);

    // Enrich the transactions that were inserted
    let inserted_transactions = sqlx_enrich_transactions(inserted_transactions, db).await?;
    // TODO: reenable when we have credits.

    info!("Enriched {} transactions for account: {}", inserted_transactions.len(), account.id);

    // Create the triggers
    for transaction in &inserted_transactions {
        sqlx_create_transaction_trigger_queue(transaction, db).await?;
    }
    Ok(inserted_transactions)
}

pub async fn sqlx_create_transaction_trigger_queue(
    transaction: &Transaction,
    db: &sqlx::MySqlPool,
) -> anyhow::Result<()> {
    let transaction_triggers: Vec<Trigger> =
        Trigger::sqlx_for_user_for_event(transaction.user_id, "transaction_created", db).await?;

    let transaction_triggers = transaction_triggers
        .into_iter()
        .filter(|trigger| trigger.filter.matches(transaction))
        .collect::<Vec<Trigger>>();

    info!("Adding {} triggers for transaction.", transaction_triggers.len());

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

pub async fn sqlx_sync_accounts(
    accounts: &mut Vec<Account>,
    db: &sqlx::MySqlPool,
) -> HashMap<u32, anyhow::Result<Vec<Transaction>>> {
    let mut import_futures = Vec::new();

    for account in accounts.iter() {
        let import_future = sqlx_import_transactions(account, db);
        import_futures.push(import_future);
    }

    let import_results = futures::future::join_all(import_futures).await;

    let mut result_map = HashMap::new();

    for (account_id, result) in import_results.into_iter().enumerate() {
        result_map.insert(accounts[account_id].id, result);
    }

    result_map
}

pub async fn sqlx_process_trigger_queue(
    queue: Vec<TriggerQueue>,
    db: &sqlx::MySqlPool,
) -> HashMap<u32, anyhow::Result<TriggerLog>> {
    let mut handles = Vec::new();

    for q in queue {
        let db = db.clone();
        let handle = thread::spawn(move || {
            let rt_handle = Runtime::new().unwrap();
            let id = q.id;
            info!("Processing trigger queue: {}", id);
            // Use the handle to the runtime to block on the future
            let result = rt_handle.block_on(async move {
                let pool = db.options().clone().connect(env::var("DATABASE_URL").unwrap().as_str()).await.unwrap();
                q.sqlx_run(&pool).await
            });
            dbg!(&result);
            info!("Processed trigger queue: {}", id);
            (id, result)
        });
        handles.push(handle);
    }

    let mut result_map = HashMap::new();
    for handle in handles {
        let (id, result) = handle.join().unwrap();
        result_map.insert(id, result);
    }

    result_map
}

pub async fn sqlx_enrich_transactions(
    transactions: Vec<Transaction>,
    db: &sqlx::MySqlPool,
) -> anyhow::Result<Vec<Transaction>> {
    let client = crate::gpt_enricher::Client::new(env::var("OPENAI_API_KEY").unwrap());
    let enriched_transactions = client
        .get_merchants(&transactions)
        .await?;
    let mut transactions: Vec<Transaction> = vec![];

    for (t_id, merchant ) in enriched_transactions {
        let mut transaction: Transaction = Transaction::sqlx_by_id(t_id, db).await?;
        match merchant.sqlx_create_or_fetch(db).await {
            Ok(merchant) => {
                transaction.merchant_id = Some(merchant.id);
                transaction.sqlx_update(db).await?;
            }
            Err(err) => {
                println!("Error creating merchant: {}", err);
                continue;
            }
        }
        transactions.push(transaction);
    }

    Ok(transactions)
}

mod tests {
    use chrono::NaiveDateTime;
    // use crate::deno::FunctionRuntime;
    use super::*;

    // #[test]
    // pub fn test_parallel() {
    //     let rt = tokio::runtime::Runtime::new().unwrap();
    //     let local = tokio::task::LocalSet::new();
    //     local.block_on(&rt, async {
    //         let mut handles = Vec::new();
    //         let rt_handle = tokio::runtime::Handle::current();

    //         let handle = thread::spawn(move || {
    //             // Use the handle to the runtime to block on the future
    //             let result = rt_handle.block_on(async {
    //                 let function = Function {
    //                     id: 1,
    //                     name: "Test".into(),
    //                     function_type: "source".into(),
    //                     source: "export default async function () {
    //                         return new Promise((resolve, reject) => {
    //                             setTimeout(() => {
    //                                 resolve('Hello');
    //                             }, 1000);
    //                         });
    //                     }"
    //                     .into(),
    //                     user_id: 1,
    //                     created_at: NaiveDateTime::from_timestamp_opt(0, 0).unwrap(),
    //                     updated_at: NaiveDateTime::from_timestamp_opt(0, 0).unwrap(),
    //                 };
    //                 let mut function_runtime = FunctionRuntime::new(&function).await.unwrap();
    //                 let r = function_runtime.run("{}", "{}").await;
    //                 r
    //             });
    //             result
    //         });
    //         handles.push(handle);

    //         for handle in handles {
    //             let result = handle.join().unwrap().unwrap();
    //             assert_eq!(result, r#""Hello""#.to_string())
    //         }
    //     });
    // }
}
