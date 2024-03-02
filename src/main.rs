use anyhow::bail;
use clap::{Parser, Subcommand};
use cli_table::{print_stdout, WithTitle};
use dotenvy::dotenv;
use nordigen::Nordigen;
use sqlx::{mysql::MySqlPoolOptions, MySqlPool};
use std::{env, time::Duration};

use crate::accounts::{get_source_account, SourceAccount};

pub use self::models::*;

pub mod accounts;
pub mod deno;
pub mod endpoints;
pub mod gpt_enricher;
pub mod models;
pub mod nordigen;
pub mod ntropy;
pub mod server;
pub mod ultrafinance;
pub mod utils;

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
        user_id: u32,
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
        account_id: u32,
        #[arg(long, value_enum)]
        format: ListFormat,
    },
    ListNordigenTransactions {
        #[arg(long)]
        account_id: u32,
        #[arg(long, value_enum)]
        format: ListFormat,
    },
    GetNordigenAccount {
        #[arg(long)]
        account_id: u32,
    },
    RenewNordigenRequisition {
        #[arg(long)]
        account_id: u32,
        #[arg(long)]
        requisition_id: Option<String>,
    },
    PopulateAccountsDetails,
    UpdateBalances,
    Add {
        #[arg(long)]
        user_id: u32,
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
        user_id: u32,
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
        user_id: u32,
    },
    Resume {
        #[arg(long)]
        requisition_id: u32,
    },
    Get {
        #[arg(long)]
        requisition_id: u32,
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
        user_id: u32,
        #[arg(long)]
        function_id: u32,
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
    ProcessSingle {
        #[arg(long)]
        id: u32,
    },
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
        account_id: u32,
    },
    CreateTrigger {
        #[arg(long)]
        id: u32,
    },
    Enrich {
        #[arg(long)]
        id: u32,
    },
    AssignMerchants {},
}

#[derive(Subcommand)]
enum ServerCommand {
    Start,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    dotenv().ok();
    env_logger::init();
    let sqlx_pool = MySqlPoolOptions::new()
        .max_connections(60)
        .acquire_timeout(Duration::from_secs(5))
        .connect(env::var("DATABASE_URL").unwrap().as_str()).await?;

    // You can check for the existence of subcommands, and if found use their
    // matches just as you would the top level cmd
    match &cli.command {
        Commands::Users(command) => match command {
            UsersCommand::List => {
                let users = User::sqlx_all(&sqlx_pool).await?;
                print_stdout(users.with_title()).unwrap_or(());
                Ok(())
            }
            UsersCommand::Add {
                name,
                email,
                password,
            } => {
                let user = NewUser {
                    name: name.clone(),
                    email: email.clone(),
                    password: password.clone(),
                }
                .sqlx_create(&sqlx_pool)
                .await?;
                dbg!(user);
                Ok(())
            }
            UsersCommand::CreateApiKey { user_id } => {
                let user: User = User::sqlx_by_id(*user_id, &sqlx_pool).await?;
                let api_key = user::create_api_key(&user, &sqlx_pool).await?;
                println!("Added API key: {}", api_key);
                Ok(())
            }
        },
        Commands::Accounts(command) => match command {
            AccountsCommand::Add {
                user_id,
                type_,
                config,
            } => {
                let source_account =
                    get_source_account(type_, config).ok_or(anyhow::anyhow!("No source found"))?;

                let mut new_account = NewAccount::from(source_account.details().await?);
                new_account.user_id = *user_id;
                new_account.config = Some(config.clone());
                new_account.account_type = type_.clone();
                let account = new_account.sqlx_create(&sqlx_pool).await?;
                dbg!(account);
                Ok(())
            }
            AccountsCommand::List => {
                let my_accounts = Account::sqlx_all(&sqlx_pool).await?;
                print_stdout(my_accounts.with_title()).unwrap_or(());
                Ok(())
            }

            AccountsCommand::ListSourceTransactions { account_id, format } => {
                let account = Account::sqlx_by_id_only(*account_id, &sqlx_pool).await?;
                let source = account.source()?;
                let transactions = source.transactions(&None, &None).await?;
                match format {
                    ListFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&transactions)?);
                    }
                    ListFormat::Table => {
                        print_stdout(transactions.with_title()).unwrap_or(());
                    }
                };
                Ok(())
            }
            AccountsCommand::ListNordigenTransactions { account_id, format } => {
                let mut client = Nordigen::new();
                let _token = client.populate_token().await?;
                let account = Account::sqlx_by_id_only(*account_id, &sqlx_pool).await?;
                let transactions = client
                    .get_account_transactions(&account.nordigen_id, &None, &None)
                    .await?;

                match format {
                    ListFormat::Json => {
                        println!("{}", serde_json::to_string_pretty(&transactions)?);
                    }
                    ListFormat::Table => {
                        print_stdout(transactions.with_title()).unwrap_or(());
                    }
                };

                Ok(())
            }
            AccountsCommand::GetNordigenAccount { account_id } => {
                let mut client = Nordigen::new();
                client.populate_token().await?;
                let account = Account::sqlx_by_id_only(*account_id, &sqlx_pool).await?;
                let _nordigen_account = client.get_account(&account.nordigen_id).await?;
                let _account_details = client.get_account_details(&account.nordigen_id).await?;
                Ok(())
            }
            AccountsCommand::RenewNordigenRequisition {
                account_id,
                requisition_id,
            } => {
                let mut client = Nordigen::new();
                client.populate_token().await?;
                let account = Account::sqlx_by_id_only(*account_id, &sqlx_pool).await?;

                let nordigen_account = client.get_account(&account.nordigen_id).await?;
                let requisition = match requisition_id {
                    Some(requisition_id) => client.get_requisition(&requisition_id).await?,
                    None => {
                        let requisition = client
                            .create_requisition(
                                &"oob://".to_owned(),
                                &nordigen_account.institution_id,
                            )
                            .await?;
                        println!("Visit {} to complete setup", requisition.link);
                        let _input: String = dialoguer::Input::new()
                            .with_prompt("Press return when completed.")
                            .interact_text()?;
                        client.get_requisition(&requisition.id).await?
                    }
                };

                if &requisition.status != "LN" {
                    bail!("Requisition not yet completed.");
                }

                let account = Account::sqlx_by_id_only(*account_id, &sqlx_pool).await?;

                for account_id in requisition.accounts {
                    let nordigen_account = client.get_account(&account_id).await?;
                    let details = nordigen_account.details().await?;
                    dbg!(&details);
                    let select_account = Account::sqlx_by_source_account_details(
                        details,
                        account.user_id,
                        &sqlx_pool,
                    )
                    .await;

                    let mut account = match select_account {
                        Ok(a) => a,
                        Err(e) => {
                            println!("Error getting account {}: {}, skipping.", account_id, e);
                            continue;
                        }
                    };
                    account.config = serde_json::to_string(&nordigen_account).ok();
                    account.sqlx_update(&sqlx_pool).await?;
                    println!("Account {} updated.", &account_id);
                }

                Ok(())
            }
            AccountsCommand::PopulateAccountsDetails => {
                let mut client = Nordigen::new();
                client.populate_token().await?;
                let accounts = Account::sqlx_all(&sqlx_pool).await?;
                for account in accounts {
                    let Ok(account_source) = account.source() else {
                        println!("Error getting account source");
                        continue;
                    };
                    let Ok(source_account) = account_source.details().await else {
                        println!("Error getting account details");
                        continue;
                    };
                    let mut update_account = UpdateAccount::from(source_account);
                    update_account.id = Some(account.id);
                    let _ = update_account.sqlx_update(&sqlx_pool).await;
                }
                Ok(())
            }
            AccountsCommand::UpdateBalances {} => {
                let mut client = Nordigen::new();
                client.populate_token().await?;
                let accounts = Account::sqlx_all(&sqlx_pool).await?;
                for mut account in accounts {
                    match account.update_balance().await {
                        Ok(_) => {
                            println!(
                                "Updated balance for account {} to {}",
                                account.id, account.balance
                            );
                            let _ = account.sqlx_update(&sqlx_pool).await;
                        }
                        Err(err) => {
                            println!("Error updating balance for account {}: {}", account.id, err)
                        }
                    }
                }
                Ok(())
            }
        },
        Commands::Functions(command) => match command {
            FunctionsCommand::List => {
                let my_functions = Function::sqlx_all(&sqlx_pool).await?;
                print_stdout(my_functions.with_title()).unwrap_or(());
                Ok(())
            }
            FunctionsCommand::Add {
                name,
                r#type,
                source,
                user_id,
            } => {
                let user = User::sqlx_by_id(*user_id, &sqlx_pool).await?;
                let new_function = NewFunction {
                    name: name.clone(),
                    function_type: r#type.clone(),
                    source: source.clone(),
                    user_id: user.id,
                };
                new_function.sqlx_create(&sqlx_pool).await?;
                Ok(())
            }
        },
        Commands::Requisitions(command) => match command {
            RequisitionsCommand::List => {
                let my_requisitions = NordigenRequisition::sqlx_all(&sqlx_pool).await?;
                print_stdout(my_requisitions.with_title()).unwrap_or(());
                Ok(())
            }
            RequisitionsCommand::ListInstitutions { country } => {
                let mut client = Nordigen::new();
                client.populate_token().await?;
                let institutions = client.get_institutions(country).await?;
                print_stdout(institutions.with_title()).unwrap_or(());
                Ok(())
            }
            RequisitionsCommand::Add {
                institution_id,
                user_id,
            } => {
                let mut client = Nordigen::new();
                client.populate_token().await?;
                let requisition = client
                    .create_requisition(&"oob://".to_owned(), institution_id)
                    .await?;
                let user = User::sqlx_by_id(*user_id, &sqlx_pool).await?;
                nordigen_requisition::sqlx_create_nordigen_requisition(
                    &requisition,
                    &user,
                    &sqlx_pool,
                )
                .await?;
                println!("Visit {} to complete setup. Once complete, run requisitions resume --requisition-id <id>", requisition.link );
                Ok(())
            }
            RequisitionsCommand::Resume { requisition_id } => {
                let mut client = Nordigen::new();
                client.populate_token().await?;
                let db_requisition =
                    NordigenRequisition::sqlx_by_id(*requisition_id, &sqlx_pool).await?;
                let requisition = client.get_requisition(&db_requisition.nordigen_id).await?;

                if &requisition.status != "LN" {
                    bail!("Requisition not yet completed.");
                }

                let user = User::sqlx_by_id(db_requisition.user_id, &sqlx_pool).await?;

                for account_id in requisition.accounts {
                    let account_details =
                        accounts::SourceAccount::details(&client.get_account(&account_id).await?)
                            .await?;
                    let mut account = NewAccount::from(account_details);
                    account.user_id = user.id;
                    let _account = account.sqlx_create(&sqlx_pool).await?;
                    println!("Account {} added.", &account_id);
                }

                Ok(())
            }
            RequisitionsCommand::Get { requisition_id } => {
                let mut client = Nordigen::new();
                client.populate_token().await?;
                let db_requisition =
                    NordigenRequisition::sqlx_by_id(*requisition_id, &sqlx_pool).await?;
                let requisition = client.get_requisition(&db_requisition.nordigen_id).await?;

                dbg!(requisition);

                Ok(())
            }
        },
        Commands::Triggers(command) => match command {
            TriggersCommand::List => {
                let triggers = Trigger::sqlx_all(&sqlx_pool).await?;
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
                let user = User::sqlx_by_id(*user_id, &sqlx_pool).await?;
                let function = Function::sqlx_by_id(*function_id, &sqlx_pool).await?;

                let trigger = NewTrigger {
                    event: event.clone(),
                    name: name.clone(),
                    filter: TriggerFilter(vec![]),
                    params: params.clone(),
                    user_id: user.id,
                    function_id: function.id,
                }
                .sqlx_create(&sqlx_pool)
                .await?;
                dbg!(trigger);
                Ok(())
            }
            TriggersCommand::Queue(command) => match command {
                TriggersQueueCommand::List => {
                    let queue = TriggerQueue::sqlx_all(&sqlx_pool).await?;
                    print_stdout(queue.with_title()).unwrap_or(());
                    Ok(())
                }
                TriggersQueueCommand::Process => {
                    let queue = TriggerQueue::sqlx_all(&sqlx_pool).await?;

                    for q in queue {
                        let id = q.id;
                        match q.sqlx_run(&sqlx_pool).await {
                            Ok(_) => println!("Processed trigger queue {}", id),
                            Err(err) => {
                                println!("Error in trigger queue {} {}", id, err.to_string())
                            }
                        }
                    }
                    Ok(())
                }
                TriggersQueueCommand::ProcessSingle { id: u32 } => {
                    let queue = TriggerQueue::sqlx_by_id(*u32, &sqlx_pool).await?;
                    dbg!(&queue);
                    match queue.sqlx_run(&sqlx_pool).await {
                        Ok(log) => {
                            println!("Processed trigger queue {}", u32);
                            dbg!(log);
                        }
                        Err(err) => println!("Error in trigger queue {} {}", u32, err.to_string()),
                    }
                    Ok(())
                }
            },
            TriggersCommand::Log(command) => match command {
                TriggersLogCommand::List => {
                    let log = TriggerLog::sqlx_all(&sqlx_pool).await?;
                    print_stdout(log.with_title()).unwrap_or(());
                    Ok(())
                }
            },
        },
        Commands::Transactions(command) => match command {
            TransactionsCommand::List => {
                let my_transactions = Transaction::sqlx_all(&sqlx_pool).await?;
                print_stdout(my_transactions.with_title()).unwrap_or(());
                Ok(())
            }
            TransactionsCommand::Import { account_id } => {
                let account = Account::sqlx_by_id_only(*account_id, &sqlx_pool).await?;

                let imported_transactions =
                    ultrafinance::sqlx_import_transactions(&account, &sqlx_pool).await?;
                println!("imported {} transactions.", imported_transactions.len());
                Ok(())
            }
            TransactionsCommand::CreateTrigger { id } => {
                let transaction = Transaction::sqlx_by_id(*id, &sqlx_pool).await?;
                ultrafinance::sqlx_create_transaction_trigger_queue(&transaction, &sqlx_pool).await
            }
            TransactionsCommand::Enrich { id } => {
                let transaction = Transaction::sqlx_by_id(*id, &sqlx_pool).await?;
                let client = gpt_enricher::Client::new(env::var("OPENAI_API_KEY").unwrap());
                dbg!(client.get_merchants(&vec![transaction]).await);

                // let enriched_transaction = client
                //     .async_enrich_transactions(vec![transaction.into()])
                //     .await?;
                // println!("Enriched transaction: {:?}", enriched_transaction);
                Ok(())
            }
            TransactionsCommand::AssignMerchants {} => {
                loop {
                    let transactions_to_do =
                        Transaction::sqlx_without_merchant_limit_100(&sqlx_pool).await?;
                    dbg!(&transactions_to_do);
                    if transactions_to_do.is_empty() {
                        break;
                    }
                    // Only take the first 10 transactions, as enrichment can timeout
                    let transactions_to_do = transactions_to_do.into_iter().take(10).collect();
                    let enriched = ultrafinance::sqlx_enrich_transactions(transactions_to_do, &sqlx_pool).await?;
                    dbg!(enriched.len());
                }

                Ok(())
            }
        },
        Commands::Merchants(command) => match command {
            MerchantsCommand::List => {
                let merchants = Merchant::sqlx_all(&sqlx_pool).await?;
                print_stdout(merchants.with_title()).unwrap_or(());
                Ok(())
            }
        },
        Commands::Server(command) => match command {
            ServerCommand::Start =>
                server::start()
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string())),
        },
    }
}
