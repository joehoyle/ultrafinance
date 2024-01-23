use crate::{
    accounts::{Amount, SourceTransaction},
    utils::display_option,
};
use anyhow::anyhow;
use chrono::{naive::NaiveDate, DateTime, Utc};
use cli_table::Table;
use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, env};

#[derive(Deserialize, Table, Serialize, ts_rs::TS, Apiv2Schema)]
#[ts(export)]
pub struct Institution {
    pub id: String,
    pub name: String,
    pub bic: String,
    #[table(skip)]
    #[allow(dead_code)]
    pub transaction_total_days: String,
    #[table(skip)]
    #[allow(dead_code)]
    pub countries: Vec<String>,
    #[table(skip)]
    #[allow(dead_code)]
    pub logo: String,
}

#[derive(Deserialize, Table, Debug, Serialize, Clone, ts_rs::TS, Apiv2Schema)]
#[ts(export)]
pub struct Requisition {
    pub id: String,
    pub status: String,
    pub redirect: String,
    #[table(skip)]
    pub accounts: Vec<String>,
    pub link: String,
}

pub struct Nordigen {
    key: String,
    secret: String,
    token: Option<AccessToken>,
    base_url: String,
}

#[derive(Deserialize)]
struct ErrorResponse {
    summary: Option<String>,
    detail: Option<String>,
    status_code: Option<i64>,
}

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
pub struct AccessToken {
    access: String,
    access_expires: i64,
    refresh: String,
    refresh_expires: i64,
}

#[derive(Deserialize, Debug, Serialize)]
#[serde(untagged)]
pub enum BankAccount {
    Iban {
        iban: String,
    },
    Bban {
        bban: String,
    },
    MaskedPan {
        #[serde(rename(deserialize = "maskedPan"))]
        masked_pan: String,
    },
    Empty {},
}

impl std::fmt::Display for BankAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                BankAccount::Iban { iban } => iban,
                BankAccount::Bban { bban } => bban,
                BankAccount::MaskedPan { masked_pan } => masked_pan,
                _ => "",
            }
        )
    }
}

#[derive(Deserialize, Debug, Clone, Serialize)]
#[allow(non_snake_case)]
pub struct CurrencyExchange {
    pub exchangeRate: String,
    pub sourceCurrency: String,
    pub targetCurrency: Option<String>,
}

impl std::fmt::Display for CurrencyExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} -> {} {}",
            self.sourceCurrency,
            self.targetCurrency.as_ref().unwrap_or(&"".to_string()),
            self.exchangeRate
        )
    }
}

#[derive(Deserialize, Table, Debug, Serialize)]
#[allow(non_snake_case)]
pub struct Transaction {
    #[table(title = "ID", display_fn = "display_option")]
    pub transactionId: Option<String>,
    #[table(title = "Debtor Name", display_fn = "display_option")]
    pub debtorName: Option<String>,
    #[table(title = "Debtor Account", display_fn = "display_option")]
    pub debtorAccount: Option<BankAccount>,
    #[table(title = "Creditor Account", display_fn = "display_option")]
    pub creditorAccount: Option<BankAccount>,
    #[table(title = "Amount", display_fn = "display_option")]
    pub transactionAmount: Option<Amount>,
    #[table(title = "Transaction Code", display_fn = "display_option")]
    pub bankTransactionCode: Option<String>,
    #[table(title = "Date", display_fn = "display_option")]
    pub bookingDate: Option<NaiveDate>,
    #[table(title = "Datetime", display_fn = "display_option")]
    pub bookingDateTime: Option<DateTime<Utc>>,
    #[table(skip)]
    pub valueDate: Option<NaiveDate>,
    #[table(title = "Information", display_fn = "display_option")]
    pub remittanceInformationUnstructured: Option<String>,
    #[table(skip)]
    pub proprietaryBankTransactionCode: Option<String>,
    #[table(title = "Creditor Name", display_fn = "display_option")]
    pub creditorName: Option<String>,
    #[table(title = "Exchange", display_fn = "display_option")]
    pub currencyExchange: Option<CurrencyExchange>,
}

#[derive(Deserialize, Debug, Clone, Serialize)]
#[allow(non_snake_case)]
pub struct Account {
    pub id: String,
    pub institution_id: String,
}

#[derive(Deserialize, Debug, Clone)]
#[allow(non_snake_case)]
pub struct AccountDetails {
    pub resourceId: String,
    pub displayName: Option<String>,
    pub iban: Option<String>,
    pub maskedPan: Option<String>,
    pub pan: Option<String>,
    pub bic: Option<String>,
    pub bban: Option<String>,
    pub currency: String,
    pub product: Option<String>,
    pub cashAccountType: Option<String>,
    pub ownerName: Option<String>,
    pub status: Option<String>,
    pub details: Option<String>,
}

#[derive(Deserialize)]
struct AccountResponse {
    account: AccountDetails,
}

#[derive(Deserialize, Debug, Clone)]
#[allow(non_snake_case)]
pub struct Balance {
    pub balanceAmount: Amount,
    pub balanceType: String,
    pub referenceDate: Option<chrono::NaiveDate>,
}

#[derive(Deserialize)]
struct BalancesResponse {
    balances: Vec<Balance>,
}

#[derive(Deserialize)]
struct Transactions {
    booked: Vec<Transaction>,
}

#[derive(Deserialize)]
struct TransactionsResponse {
    transactions: Transactions,
}

impl Nordigen {
    pub fn new() -> Self {
        Self {
            key: env::var("NORDIGEN_SECRET_ID").unwrap().into(),
            secret: env::var("NORDIGEN_SECRET_KEY").unwrap().into(),
            token: None,
            base_url: "https://ob.gocardless.com/api/v2".into(),
        }
    }

    pub fn get_institutions(&self, country: &Option<String>) -> anyhow::Result<Vec<Institution>> {
        let mut args: HashMap<String, String> = HashMap::new();
        if country.is_some() {
            args.insert("country".into(), country.as_ref().unwrap().clone());
        }
        let response = self.request(reqwest::Method::GET, "/institutions/", Some(&args))?;
        response
            .json::<Vec<Institution>>()
            .map_err(anyhow::Error::msg)
    }

    pub fn get_institution(&self, id: &String) -> anyhow::Result<Institution> {
        let response = self.request(
            reqwest::Method::GET,
            format!("/institutions/{}", id).as_str(),
            None,
        )?;
        response.json::<Institution>().map_err(anyhow::Error::msg)
    }

    pub fn get_account(&self, id: &String) -> anyhow::Result<Account> {
        let json = self
            .request(
                reqwest::Method::GET,
                format!("/accounts/{}", id).as_str(),
                None,
            )?
            .text()?;

        let response = serde_json::from_str::<Account>(json.as_str())?;
        Ok(response)
    }

    pub fn get_account_details(&self, id: &String) -> anyhow::Result<AccountDetails> {
        let json = self
            .request(
                reqwest::Method::GET,
                format!("/accounts/{}/details", id).as_str(),
                None,
            )?
            .text()?;
        dbg!(json.as_str());
        let response = serde_json::from_str::<AccountResponse>(json.as_str())?;
        Ok(response.account)
    }

    pub fn get_account_balances(&self, id: &String) -> anyhow::Result<Vec<Balance>> {
        let json = self
            .request(
                reqwest::Method::GET,
                format!("/accounts/{}/balances", id).as_str(),
                None,
            )?
            .text()?;

        let response = serde_json::from_str::<BalancesResponse>(json.as_str())?;
        Ok(response.balances)
    }

    pub fn get_account_transactions(
        &self,
        id: &String,
        date_from: &Option<NaiveDate>,
        date_to: &Option<NaiveDate>,
    ) -> anyhow::Result<Vec<Transaction>> {
        let mut args: HashMap<String, String> = HashMap::new();
        if date_from.is_some() {
            args.insert(
                "date_from".into(),
                date_from.as_ref().unwrap().format("%Y-%m-%d").to_string(),
            );
        }
        if date_to.is_some() {
            args.insert(
                "date_to".into(),
                date_to.as_ref().unwrap().format("%Y-%m-%d").to_string(),
            );
        }
        let transactions = self
            .request(
                reqwest::Method::GET,
                format!("/accounts/{}/transactions/", id).as_str(),
                Some(&args),
            )?
            .text()?;
        println!("{}", &transactions);
        let transactions: TransactionsResponse = serde_json::from_str(transactions.as_str())?;
        let transactions = transactions
            .transactions
            .booked
            .into_iter()
            .map(|mut transaction| {
                if transaction.transactionId.is_none() {
                    let mut hasher = Sha256::new();
                    let date = match transaction.bookingDate.as_ref() {
                        Some(d) => d.format("%Y-%m-%d").to_string(),
                        None => "".into(),
                    };
                    let hash_string = format!(
                        "{}:{}:{}:{}",
                        date,
                        date,
                        transaction
                            .remittanceInformationUnstructured
                            .as_ref()
                            .unwrap_or(&"".into()),
                        "[object Object]"
                    );
                    hasher.update(hash_string);
                    let hash = hasher.finalize();
                    let hash = format!("{:x}", hash);
                    transaction.transactionId = Some(hash);
                }
                transaction
            })
            .collect();
        Ok(transactions)
    }

    pub fn create_requisition(
        &self,
        redirect: &String,
        institution_id: &String,
    ) -> anyhow::Result<Requisition> {
        let mut args: HashMap<String, String> = HashMap::new();
        args.insert("redirect".into(), redirect.clone());
        args.insert("institution_id".into(), institution_id.clone());
        // args.insert("account_selection".into(), "true".into());

        self.request(reqwest::Method::POST, "/requisitions/", Some(&args))?
            .json::<Requisition>()
            .map_err(anyhow::Error::msg)
    }

    pub fn get_requisition(&self, id: &String) -> anyhow::Result<Requisition> {
        self.request(
            reqwest::Method::GET,
            format!("/requisitions/{}", id).as_str(),
            None,
        )?
        .json::<Requisition>()
        .map_err(anyhow::Error::msg)
    }

    pub fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        args: Option<&HashMap<String, String>>,
    ) -> anyhow::Result<reqwest::blocking::Response> {
        let request = reqwest::blocking::Client::new();
        let mut request = match method {
            reqwest::Method::GET => {
                let mut request = request.get(format!("{}{}", self.base_url, path));
                if args.is_some() {
                    request = request.query(&args);
                }
                request
            }
            reqwest::Method::POST => {
                let mut request = request.post(format!("{}{}", self.base_url, path));
                if args.is_some() {
                    request = request.json(&args);
                }
                request
            }
            _ => return Err(anyhow!("Method not supported")),
        };

        if self.token.is_some() {
            request = request.header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.token.as_ref().unwrap().access.clone()),
            );
        }
        match request.send() {
            Ok(r) => {
                if r.status().is_success() {
                    return Ok(r);
                }
                let error = r.json::<ErrorResponse>()?;
                Err(anyhow::Error::msg(format!(
                    "{}. {} {}",
                    error.summary.unwrap_or("No summary".into()),
                    error.detail.unwrap_or("".into()),
                    error.status_code.unwrap_or(0)
                )))
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn populate_token(&mut self) -> anyhow::Result<AccessToken> {
        let mut args: HashMap<String, String> = HashMap::new();
        args.insert("secret_id".into(), self.key.clone());
        args.insert("secret_key".into(), self.secret.clone());

        let token = self.request(reqwest::Method::POST, "/token/new/", Some(&args))?;
        let token = token.json::<AccessToken>()?;
        self.token = Some(token.clone());
        Ok(token)
    }
}

impl From<Transaction> for SourceTransaction {
    fn from(transaction: Transaction) -> Self {
        Self {
            id: transaction.transactionId.unwrap(),
            creditor_name: transaction.creditorName,
            debtor_name: transaction.debtorName,
            remittance_information: transaction.remittanceInformationUnstructured,
            booking_date: transaction.bookingDate.unwrap(),
            booking_datetime: transaction.bookingDateTime.map(|d| d.naive_utc()),
            transaction_amount: transaction.transactionAmount.unwrap(),
            currency_exchange_rate: transaction
                .currencyExchange
                .as_ref()
                .map(|c| c.exchangeRate.clone()),
            proprietary_bank_transaction_code: transaction.proprietaryBankTransactionCode,
            currency_exchange_source_currency: transaction
                .currencyExchange
                .as_ref()
                .map(|c| c.sourceCurrency.clone()),
            currency_exchange_target_currency: transaction
                .currencyExchange
                .as_ref()
                .and_then(|c| c.targetCurrency.clone()),
        }
    }
}

impl crate::accounts::SourceAccount for Account {
    fn details(&self) -> Result<crate::accounts::SourceAccountDetails, anyhow::Error> {
        let mut client = Nordigen::new();
        client.populate_token()?;
        let account = client.get_account(&self.id)?;
        let account_details = client.get_account_details(&self.id)?;
        let institution = client.get_institution(&account.institution_id)?;

        let number = account_details
            .iban
            .or(account_details.bban)
            .or(account_details.bic)
            .or(account_details.pan)
            .or(account_details.maskedPan);

        Ok(crate::accounts::SourceAccountDetails {
            id: self.id.clone(),
            number: number.unwrap_or("".into()),
            currency: account_details.currency,
            details: account_details.details.unwrap_or("".into()),
            owner_name: account_details.ownerName,
            icon: Some(institution.logo),
            institution_name: institution.name,
        })
    }

    fn balance(&self) -> Result<crate::accounts::Amount, anyhow::Error> {
        let mut client = Nordigen::new();
        client.populate_token().unwrap();
        Ok(client
            .get_account_balances(&self.id)?
            .first().ok_or(anyhow!("Account not found"))?
            .balanceAmount
            .clone())
    }

    fn transactions(
        &self,
        date_from: &Option<NaiveDate>,
        date_to: &Option<NaiveDate>,
    ) -> Result<Vec<SourceTransaction>, anyhow::Error> {
        let mut client = Nordigen::new();
        client.populate_token().unwrap();
        let transactions = client.get_account_transactions(&self.id, date_from, date_to)?;
        Ok(transactions.into_iter().map(|t| t.into()).collect())
    }
}
