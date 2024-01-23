use anyhow::bail;
use clap::{Parser, Subcommand};
use cli_table::{print_stdout, WithTitle};
use diesel::mysql::MysqlConnection;
use diesel::{QueryDsl, RunQueryDsl};
use dotenvy::dotenv;
use nordigen::Nordigen;
use std::env;
use ultrafinance::DbConnection;

use crate::accounts::{get_source_account, SourceAccount};

pub use self::models::*;

pub mod deno;
pub mod models;
pub mod nordigen;
pub mod schema;
pub mod ultrafinance;
pub mod utils;
pub mod server;
pub mod endpoints;
pub mod accounts;
pub mod ntropy;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Users commands
    #[command(subcommand)]
    Users(UsersCommand),
    /// Accounts commands
    #[command(subcommand)]
    Accounts(AccountsCommand),
    /// Functions commands
    #[command(subcommand)]
    Functions(FunctionsCommand),
    /// Requisitions commands
    #[command(subcommand)]
    Requisitions(RequisitionsCommand),
    /// Transactions commands
    #[command(subcommand)]
    Transactions(TransactionsCommand),
    /// Triggers commands
    #[command(subcommand)]
    Triggers(TriggersCommand),
    #[command(subcommand)]
    Merchants(MerchantsCommand),
    #[command(subcommand)]
    Server(ServerCommand),
}

#[derive(Subcommand)]
enum UsersCommand {
    List,
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        email: String,
        #[arg(long)]
        password: String,
    },
    CreateApiKey {
        #[arg(long)]
        user_id: i32,
    },
}

#[derive(Clone, clap::ValueEnum)]
enum ListFormat {
    Json,
    Table,
}

#[derive(Subcommand)]
enum AccountsCommand {
    List,
    ListSourceTransactions {
        #[arg(long)]
        account_id: i32,
        #[arg(long, value_enum)]
        format: ListFormat,
    },
    ListNordigenTransactions {
        #[arg(long)]
        account_id: i32,
        #[arg(long, value_enum)]
        format: ListFormat,
    },
    GetNordigenAccount {
        #[arg(long)]
        account_id: i32,
    },
    RenewNordigenRequisition {
        #[arg(long)]
        account_id: i32,
        #[arg(long)]
        requisition_id: Option<String>,
    },
    PopulateAccountsDetails,
    UpdateBalances,
    Add {
        #[arg(long)]
        user_id: i32,
        #[arg(long, name = "type")]
        type_: String,
        #[arg(long)]
        config: String,
    },
}

#[derive(Subcommand)]
enum FunctionsCommand {
    List,
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        r#type: String,
        #[arg(long)]
        source: String,
        #[arg(long)]
        user_id: i32,
    },
}

#[derive(Subcommand)]
enum RequisitionsCommand {
    List,
    ListInstitutions {
        #[arg(long)]
        country: Option<String>,
    },
    Add {
        #[arg(long)]
        institution_id: String,
        #[arg(long)]
        user_id: i32,
    },
    Resume {
        #[arg(long)]
        requisition_id: i32,
    },
    Get {
        #[arg(long)]
        requisition_id: i32,
    },
}

#[derive(Subcommand)]
enum TriggersCommand {
    List,
    Add {
        #[arg(long)]
        event: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        params: String,
        #[arg(long)]
        user_id: i32,
        #[arg(long)]
        function_id: i32,
    },
    #[command(subcommand)]
    Queue(TriggersQueueCommand),
    #[command(subcommand)]
    Log(TriggersLogCommand),
}

#[derive(Subcommand)]
enum MerchantsCommand {
    List,
}

#[derive(Subcommand)]
enum TriggersQueueCommand {
    List,
    Process,
}

#[derive(Subcommand)]
enum TriggersLogCommand {
    List,
}

#[derive(Subcommand)]
enum TransactionsCommand {
    List,
    Import {
        #[arg(long)]
        account_id: i32,
    },
    CreateTrigger {
        #[arg(long)]
        id: i32,
    },
    Enrich {
        #[arg(long)]
        id: i32,
    },
    AssignMerchants {},
}

#[derive(Subcommand)]
enum ServerCommand {
    Start
}

fn establish_connection() -> DbConnection {
    // MysqlConnection::establish(env::var("DATABASE_URL").unwrap().as_str()).unwrap()
    let manager = diesel::r2d2::ConnectionManager::<MysqlConnection>::new(
        env::var("DATABASE_URL").unwrap().as_str(),
    );
    let pool = diesel::r2d2::Pool::builder().build(manager).unwrap();
    let con = pool.get().map_err(anyhow::Error::msg).unwrap();
    con
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    dotenv().ok();

    // You can check for the existence of subcommands, and if found use their
    // matches just as you would the top level cmd
    match &cli.command {
        Commands::Users(command) => match command {
            UsersCommand::List => {
                let mut con = establish_connection();
                use self::schema::users::dsl::*;
                let my_users: Vec<User> = users.load::<User>(&mut con).unwrap();
                print_stdout(my_users.with_title()).unwrap_or(());
                Ok(())
            }
            UsersCommand::Add {
                name,
                email,
                password,
            } => {
                let mut con = establish_connection();
                let user = NewUser {
                    name: name.clone(),
                    email: email.clone(),
                    password: password.clone(),
                }
                .create(&mut con)?;
                dbg!(user);
                Ok(())
            }
            UsersCommand::CreateApiKey { user_id } => {
                let mut con = establish_connection();
                let user: User = schema::users::dsl::users.find(user_id).first(&mut con)?;
                let api_key = user::create_api_key(&user, &mut con)?;
                println!("Added API key: {}", api_key);
                Ok(())
            }
        },
        Commands::Accounts(command) => match command {
            AccountsCommand::Add { user_id, type_, config } => {
                let source_account = get_source_account(type_, config).ok_or(anyhow::anyhow!("No source found"))?;

                let mut new_account = NewAccount::from(source_account.details()?);
                new_account.user_id = *user_id;
                new_account.config = Some(config.clone());
                new_account.account_type = type_.clone();
                let mut con = establish_connection();
                let account = new_account.create(&mut con)?;
                dbg!(account);
                Ok(())
            },
            AccountsCommand::List => {
                let mut con = establish_connection();
                use self::schema::accounts::dsl::*;
                let my_accounts: Vec<Account> = accounts.load::<Account>(&mut con).unwrap();
                print_stdout(my_accounts.with_title()).unwrap_or(());
                Ok(())
            }
            AccountsCommand::ListSourceTransactions { account_id, format } => {
                let mut con = establish_connection();
                let account: Account = schema::accounts::dsl::accounts
                    .find(account_id)
                    .first(&mut con)?;
                let source = account.source()?;
                let transactions = source.transactions(&None, &None)?;
                match format {
                    ListFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&transactions)?);
                    },
                    ListFormat::Table => {
                        print_stdout(transactions.with_title()).unwrap_or(());
                    },
                };
                Ok(())
            }
            AccountsCommand::ListNordigenTransactions { account_id, format } => {
                let mut client = Nordigen::new();
                let mut con = establish_connection();
                let _token = client.populate_token()?;
                let account: Account = schema::accounts::dsl::accounts
                    .find(account_id)
                    .first(&mut con)?;
                let transactions =
                    client.get_account_transactions(&account.nordigen_id, &None, &None)?;

                match format {
                    ListFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&transactions)?);
                    },
                    ListFormat::Table => {
                        print_stdout(transactions.with_title()).unwrap_or(());
                    },
                };

                Ok(())
            }
            AccountsCommand::GetNordigenAccount { account_id } => {
                let mut client = Nordigen::new();
                let mut con = establish_connection();
                client.populate_token()?;
                let account: Account = schema::accounts::dsl::accounts
                    .find(account_id)
                    .first(&mut con)?;

                let _nordigen_account = client.get_account(&account.nordigen_id)?;
                let _account_details = client.get_account_details(&account.nordigen_id)?;
                Ok(())
            }
            AccountsCommand::RenewNordigenRequisition { account_id, requisition_id } => {
                let mut client = Nordigen::new();
                client.populate_token()?;
                let mut con = establish_connection();
                let account: Account = schema::accounts::dsl::accounts
                    .find(account_id)
                    .first(&mut con)?;

                let nordigen_account = client.get_account(&account.nordigen_id)?;
                let requisition = match requisition_id {
                    Some(requisition_id) => client.get_requisition(&requisition_id)?,
                    None => {
                        let requisition = client.create_requisition(&"oob://".to_owned(), &nordigen_account.institution_id)?;
                        println!("Visit {} to complete setup", requisition.link );
                        let _input: String = dialoguer::Input::new().with_prompt("Press return when completed.").interact_text()?;
                        client.get_requisition(&requisition.id)?

                    },
                };

                if &requisition.status != "LN" {
                    bail!("Requisition not yet completed.");
                }

                let account: Account = schema::accounts::dsl::accounts.find(account_id).first(&mut con)?;

                for account_id in requisition.accounts {
                    let nordigen_account = client.get_account(&account_id)?;
                    let details = nordigen_account.details()?;
                    dbg!(&details);
                    let select_account = Account::by_source_account_details(details, account.user_id);

                    let mut account = match select_account.first::<Account>(&mut con) {
                        Ok(a) => a,
                        Err(e) => {
                            println!("Error getting account {}: {}, skipping.", account_id, e);
                            continue;
                        }
                    };
                    account.config = serde_json::to_string(&nordigen_account).ok();
                    account.update(&mut con)?;
                    println!("Account {} updated.", &account_id);
                }

                Ok(())
            }
            AccountsCommand::PopulateAccountsDetails => {
                let mut client = Nordigen::new();
                client.populate_token()?;
                let mut con = establish_connection();
                use diesel::*;
                let accounts = Account::all().load::<Account>(&mut con)?;
                for account in accounts {
                    let Ok(account_source) = account.source() else {
                        println!("Error getting account source");
                        continue;
                    };
                    let Ok(source_account) = account_source.details() else {
                        println!("Error getting account details");
                        continue;
                    };
                    let mut update_account = UpdateAccount::from(source_account);
                    update_account.id = Some(account.id);
                    let _ = update_account.update(&mut con);
                }
                Ok(())
            }
            AccountsCommand::UpdateBalances {} => {
                let mut client = Nordigen::new();
                client.populate_token()?;
                let mut con = establish_connection();
                use diesel::*;
                let accounts = Account::all().load(&mut con)?;
                for mut account in accounts {
                    match account.update_balance() {
                        Ok(_) => {
                            println!("Updated balance for account {} to {}", account.id, account.balance);
                            let _ = account.update(&mut con);
                        },
                        Err(err) => println!("Error updating balance for account {}: {}", account.id, err),
                    }
                }
                Ok(())
            },
        },
        Commands::Functions(command) => match command {
            FunctionsCommand::List => {
                let mut con = establish_connection();
                use self::schema::functions::dsl::*;
                let my_functions: Vec<Function> = functions.load::<Function>(&mut con).unwrap();
                print_stdout(my_functions.with_title()).unwrap_or(());
                Ok(())
            }
            FunctionsCommand::Add {
                name,
                r#type,
                source,
                user_id,
            } => {
                let mut con = establish_connection();
                let user: User = schema::users::dsl::users.find(user_id).first(&mut con)?;
                let new_function = NewFunction {
                    name: name.clone(),
                    function_type: r#type.clone(),
                    source: source.clone(),
                    user_id: user.id,
                };
                new_function.create(&mut con)?;
                Ok(())
            }
        },
        Commands::Requisitions(command) => match command {
            RequisitionsCommand::List => {
                let mut con = establish_connection();
                use self::schema::nordigen_requisitions::dsl::*;
                let my_requisitions: Vec<NordigenRequisition> = nordigen_requisitions
                    .load::<NordigenRequisition>(&mut con)
                    .unwrap();
                print_stdout(my_requisitions.with_title()).unwrap_or(());
                Ok(())
            }
            RequisitionsCommand::ListInstitutions { country } => {
                let mut client = Nordigen::new();
                client.populate_token()?;
                let institutions = client.get_institutions(country)?;
                print_stdout(institutions.with_title()).unwrap_or(());
                Ok(())
            }
            RequisitionsCommand::Add {
                institution_id,
                user_id,
            } => {
                let mut client = Nordigen::new();
                client.populate_token()?;
                let requisition =
                    client.create_requisition(&"oob://".to_owned(), institution_id)?;
                let mut con = establish_connection();
                use self::schema::users::dsl::*;
                let user = users.find(user_id).first(&mut con)?;
                nordigen_requisition::create_nordigen_requisition(&requisition, &user, &mut con)?;
                println!("Visit {} to complete setup. Once complete, run requisitions resume --requisition-id <id>", requisition.link );
                Ok(())
            }
            RequisitionsCommand::Resume { requisition_id } => {
                let mut client = Nordigen::new();
                client.populate_token()?;
                let mut con = establish_connection();
                use self::schema::nordigen_requisitions::dsl::*;
                let db_requisition: NordigenRequisition =
                    nordigen_requisitions.find(requisition_id).first(&mut con)?;
                let requisition = client.get_requisition(&db_requisition.nordigen_id)?;

                if &requisition.status != "LN" {
                    bail!("Requisition not yet completed.");
                }

                let user: User = schema::users::dsl::users
                    .find(db_requisition.user_id)
                    .first(&mut con)?;

                for account_id in requisition.accounts {
                    let account_details = accounts::SourceAccount::details(&client.get_account(&account_id)?)?;
                    let mut account = NewAccount::from(account_details);
                    account.user_id = user.id;
                    let _account = account.create(&mut con)?;
                    println!("Account {} added.", &account_id);
                }

                Ok(())
            },
            RequisitionsCommand::Get { requisition_id } => {
                let mut client = Nordigen::new();
                client.populate_token()?;
                let mut con = establish_connection();
                use self::schema::nordigen_requisitions::dsl::*;
                let db_requisition: NordigenRequisition =
                    nordigen_requisitions.find(requisition_id).first(&mut con)?;
                let requisition = client.get_requisition(&db_requisition.nordigen_id)?;

                dbg!(requisition);

                Ok(())
            }
        },
        Commands::Triggers(command) => match command {
            TriggersCommand::List => {
                let mut con = establish_connection();
                let triggers: Vec<Trigger> = schema::triggers::dsl::triggers
                    .load::<Trigger>(&mut con)
                    .unwrap();
                dbg!(&triggers);
                print_stdout(triggers.with_title()).unwrap_or(());
                Ok(())
            }
            TriggersCommand::Add {
                event,
                name,
                params,
                user_id,
                function_id,
            } => {
                let mut con = establish_connection();
                let user: User = schema::users::dsl::users.find(user_id).first(&mut con)?;
                let function: Function = schema::functions::dsl::functions
                    .find(function_id)
                    .first(&mut con)?;

                let trigger = NewTrigger {
                    event: event.clone(),
                    name: name.clone(),
                    filter: TriggerFilter(vec![]),
                    params: params.clone(),
                    user_id: user.id,
                    function_id: function.id,
                }
                .create(&mut con)?;
                dbg!(trigger);
                Ok(())
            }
            TriggersCommand::Queue(command) => match command {
                TriggersQueueCommand::List => {
                    let mut con = establish_connection();
                    let queue: Vec<TriggerQueue> = schema::trigger_queue::dsl::trigger_queue
                        .load::<TriggerQueue>(&mut con)
                        .unwrap();
                    print_stdout(queue.with_title()).unwrap_or(());
                    Ok(())
                }
                TriggersQueueCommand::Process => {
                    let mut con = establish_connection();
                    let queue: Vec<TriggerQueue> = schema::trigger_queue::dsl::trigger_queue
                        .load::<TriggerQueue>(&mut con)
                        .unwrap();

                    for q in queue {
                        match q.run(&mut con) {
                            Ok(_) => println!("Processed trigger queue {}", q.id),
                            Err(err) => {
                                println!("Error in trigger queue {} {}", q.id, err.to_string())
                            }
                        }
                    }
                    Ok(())
                }
            },
            TriggersCommand::Log(command) => match command {
                TriggersLogCommand::List => {
                    let mut con = establish_connection();
                    let log: Vec<TriggerLog> = schema::trigger_log::dsl::trigger_log
                        .load::<TriggerLog>(&mut con)
                        .unwrap();
                    print_stdout(log.with_title()).unwrap_or(());
                    Ok(())
                }
            },
        },
        Commands::Transactions(command) => match command {
            TransactionsCommand::List => {
                let mut con = establish_connection();
                use self::schema::transactions::dsl::*;
                let my_transactions: Vec<Transaction> =
                    transactions.load::<Transaction>(&mut con).unwrap();
                print_stdout(my_transactions.with_title()).unwrap_or(());
                Ok(())
            }
            TransactionsCommand::Import { account_id } => {
                let mut con = establish_connection();
                use diesel::*;

                 let account: Account = schema::accounts::dsl::accounts
                    .find(account_id)
                    .first(&mut con)?;

                let imported_transactions = ultrafinance::import_transactions(&account, &mut con)?;
                println!("imported {} transactions.", imported_transactions.len());
                Ok(())
            }
            TransactionsCommand::CreateTrigger { id } => {
                let mut con = establish_connection();
                let transaction: Transaction = self::schema::transactions::dsl::transactions
                    .find(id)
                    .first(&mut con)?;

                ultrafinance::create_transaction_trigger_queue(&transaction, &mut con)
            }
            TransactionsCommand::Enrich { id } => {
                let mut con = establish_connection();
                let transaction: Transaction = self::schema::transactions::dsl::transactions
                    .find(id)
                    .first(&mut con)?;

                let client = ntropy::ApiClient::new(env::var("NTROPY_API_KEY").unwrap());
                let enriched_transaction = client.enrich_transactions(vec![transaction.into()])?;
                println!("Enriched transaction: {:?}", enriched_transaction);
                Ok(())
            }
            TransactionsCommand::AssignMerchants {} => {
                use self::schema::transactions::dsl::*;
                use diesel::*;
                let mut con = establish_connection();
                let transactions_to_do: Vec<Transaction> = Transaction::all()
                    .filter(merchant_id.is_null()).order(id.desc()).limit(200).load(&mut con)?;

                let client = ntropy::ApiClient::new(env::var("NTROPY_API_KEY").unwrap());
                // Call .into() on all transactions_to_do
                let enriched_transactions = client.enrich_transactions(transactions_to_do.into_iter().map(|t| t.into()).collect())?;
                for enriched_transaction in enriched_transactions {
                    match NewMerchant::try_from(&enriched_transaction) {
                        Ok(merchant) => {
                            match merchant.create_or_fetch(&mut con) {
                                Ok(merchant) => {
                                    let t_id = enriched_transaction.transaction_id.parse::<i32>().unwrap();
                                    let mut transaction: Transaction = Transaction::by_id_only(t_id).first(&mut con)?;
                                    transaction.merchant_id = Some(merchant.id);
                                    transaction.update(&mut con)?;
                                },
                                Err(err) => {
                                    println!("Error creating merchant: {}", err);
                                    continue;
                                }
                            }
                        },
                        Err(err) => println!("Error getting merchant: {}", err),
                    }
                }
                Ok(())
            }
        },
        Commands::Merchants(command) => match command {
            MerchantsCommand::List => {
                let merchants = Merchant::all().load::<Merchant>(&mut establish_connection())?;
                print_stdout(merchants.with_title()).unwrap_or(());
                Ok(())
            }
        }
        Commands::Server(command) => match command {
            ServerCommand::Start => {
                server::start().map_err(|e|anyhow::anyhow!(e.to_string()))
            },
        },
    }
}
