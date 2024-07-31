use crate::accounts::SourceAccount;
use crate::{models::*, synth_api};
use anyhow::anyhow;
use async_trait::async_trait;
use chrono::Duration;
use log::info;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash;
use std::{env, thread};
use tokio::runtime::Runtime;

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

    info!(
        "Date of latest transaction for account minus 1 week to account for pending: {:?}",
        from_date
    );

    let account_clone = account.clone();
    let other_transactions = account_clone
        .source()?
        .transactions(&from_date, &None)
        .await?;

    info!(
        "Found {} transactions for account: {}",
        other_transactions.len(),
        account.id
    );

    let mut new_transactions: Vec<transaction::NewTransaction> = vec![];
    for transaction in other_transactions {
        let exists = sqlx::query_scalar!(
            "SELECT EXISTS(SELECT 1 FROM transactions WHERE external_id = ? AND account_id = ?)",
            &transaction.id,
            &account.id
        )
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

    info!(
        "Sucessfully imported {} transactions for account: {}",
        inserted_transactions.len(),
        account.id
    );

    // Enrich the transactions that were inserted
    let inserted_transactions = sqlx_enrich_transactions(inserted_transactions, db).await?;
    // TODO: reenable when we have credits.

    info!(
        "Enriched {} transactions for account: {}",
        inserted_transactions.len(),
        account.id
    );

    // Create the triggers
    for transaction in &inserted_transactions {
        run_triggers_for_transaction(transaction, db).await?;
    }
    Ok(inserted_transactions)
}

pub async fn run_triggers_for_transaction(
    transaction: &Transaction,
    db: &sqlx::MySqlPool,
) -> anyhow::Result<()> {
    let transaction_triggers: Vec<Trigger> =
        Trigger::sqlx_for_user_for_event(transaction.user_id, "transaction_created", db).await?;

    let transaction_triggers = transaction_triggers
        .into_iter()
        .filter(|trigger| trigger.filter.matches(transaction))
        .collect::<Vec<Trigger>>();

    info!(
        "Adding {} triggers for transaction.",
        transaction_triggers.len()
    );

    use futures::stream::{self, StreamExt};

    stream::iter(&transaction_triggers)
        .for_each_concurrent(None, |trigger| async move {
            if let Err(e) = trigger.sqlx_run(transaction, db).await {
                eprintln!("Failed to run trigger: {:?}", e);
            }
        })
        .await;
    Ok(())
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

pub async fn sqlx_enrich_transactions(
    transactions: Vec<Transaction>,
    db: &sqlx::MySqlPool,
) -> anyhow::Result<Vec<Transaction>> {
    let enriched_transactions = synth_api::Client::new(env::var("SYNTH_API_KEY").unwrap())
        .get_merchants(&transactions)
        .await?;
    let mut returned_transactions: Vec<Transaction> = vec![];
    let mut matched_enriched_transactions: Vec<u32> = vec![];

    for (t_id, merchant) in enriched_transactions {
        let mut transaction: Transaction = Transaction::sqlx_by_id(t_id, db).await?;
        matched_enriched_transactions.push(t_id);
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
        returned_transactions.push(transaction);
    }

    // Push any transactions we were passed that didn't get enriched
    for transaction in transactions {
        if matched_enriched_transactions.contains(&transaction.id) {
            continue;
        }
        returned_transactions.push(transaction);
    }

    Ok(returned_transactions)
}

mod tests {

    // use crate::deno::FunctionRuntime;

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

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct Currency(iso_currency::Currency);

impl Display for Currency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.code())
    }
}

impl From<String> for Currency {
    fn from(s: String) -> Self {
        Currency(iso_currency::Currency::from_code(&s).unwrap_or(iso_currency::Currency::USD))
    }
}

impl From<Currency> for String {
    fn from(c: Currency) -> Self {
        c.to_string()
    }
}

impl Hash for Currency {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.code().hash(state);
    }
}

impl<'r> sqlx::Decode<'r, sqlx::MySql> for Currency {
    fn decode(
        value: <sqlx::MySql as sqlx::database::HasValueRef<'r>>::ValueRef,
    ) -> std::result::Result<Currency, Box<(dyn std::error::Error + Send + Sync + 'static)>> {
        let s: String = sqlx::Decode::<'r, sqlx::MySql>::decode(value)?;
        Currency::try_from(s).map_err(|e| e.into())
    }
}

impl<'a> sqlx::Encode<'a, sqlx::MySql> for Currency {
    fn size_hint(&self) -> usize {
        0
    }

    fn encode_by_ref(
        &self,
        buf: &mut <sqlx::MySql as sqlx::database::HasArguments<'a>>::ArgumentBuffer,
    ) -> sqlx::encode::IsNull {
        sqlx::Encode::<'a, sqlx::MySql>::encode_by_ref(&self.0.code(), buf)
    }
}

impl sqlx::Type<sqlx::MySql> for Currency {
    fn type_info() -> <sqlx::MySql as sqlx::Database>::TypeInfo {
        <String as sqlx::Type<sqlx::MySql>>::type_info()
    }
}

impl Currency {
    fn try_from(s: String) -> Result<Self, anyhow::Error> {
        Ok(Currency(
            iso_currency::Currency::from_code(&s).ok_or(anyhow!("Invalid currency code."))?,
        ))
    }

    pub fn used_by(&self) -> Vec<&str> {
        self.0.used_by().iter().map(|c| c.name()).collect()
    }
}

#[async_trait]
pub trait TransactionDestination {
    // fn new(params: &str) -> Result<Self, anyhow::Error> where Self: Sized;
    async fn transaction_created(&self, transaction: &Transaction) -> Result<(), anyhow::Error>;
    // async fn get_params() -> Result<FunctionParams, anyhow::Error>;
}
