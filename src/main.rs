use anyhow::bail;
use clap::{Parser, Subcommand};
use cli_table::{print_stdout, WithTitle};
use diesel::mysql::MysqlConnection;
use diesel::{QueryDsl, RunQueryDsl};
use dotenvy::dotenv;
use nordigen::Nordigen;
use std::env;
use ultrafinance::DbConnection;

pub use self::models::*;

pub mod deno;
pub mod models;
pub mod nordigen;
pub mod schema;
pub mod ultrafinance;
pub mod utils;
pub mod server;
pub mod endpoints;

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

#[derive(Subcommand)]
enum AccountsCommand {
    List,
    ListNordigenTransactions {
        #[arg(long)]
        account_id: i32,
    },
    GetNordigenAccount {
        #[arg(long)]
        account_id: i32,
    },
    GetNordigenAccountBalances {
        #[arg(long)]
        account_id: i32,
    },
    PopulateAccountsDetails,
    UpdateBalances,
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
            AccountsCommand::List => {
                let mut con = establish_connection();
                use self::schema::accounts::dsl::*;
                let my_accounts: Vec<Account> = accounts.load::<Account>(&mut con).unwrap();
                print_stdout(my_accounts.with_title()).unwrap_or(());
                Ok(())
            }
            AccountsCommand::ListNordigenTransactions { account_id } => {
                let mut client = Nordigen::new();
                let mut con = establish_connection();
                let token = client.populate_token()?;
                dbg!(token);
                let account: Account = schema::accounts::dsl::accounts
                    .find(account_id)
                    .first(&mut con)?;
                let transactions =
                    client.get_account_transactions(&account.nordigen_id, &None, &None)?;
                print_stdout(transactions.with_title()).unwrap_or(());
                Ok(())
            }
            AccountsCommand::GetNordigenAccount { account_id } => {
                let mut client = Nordigen::new();
                let mut con = establish_connection();
                client.populate_token()?;
                let account: Account = schema::accounts::dsl::accounts
                    .find(account_id)
                    .first(&mut con)?;
                let account = client.get_account_details(&account.nordigen_id)?;

                dbg!(account);
                Ok(())
            }
            AccountsCommand::PopulateAccountsDetails => {
                let mut client = Nordigen::new();
                client.populate_token()?;
                let mut con = establish_connection();
                use diesel::*;
                let accounts = Account::all().load(&mut con)?;
                for account in accounts {
                    let nordigen_details = client.get_account_details(&account.nordigen_id)?;
                    let nordigen_account = client.get_account(&account.nordigen_id)?;
                    let nordigen_institution = client.get_institution(&nordigen_account.institution_id)?;
                    {
                        UpdateAccount {
                            id: Some(account.id),
                            name: None,
                            iban: nordigen_details.iban,
                            bic: nordigen_details.bic,
                            bban: nordigen_details.bban,
                            pan: nordigen_details.pan,
                            currency: nordigen_details.currency,
                            product: nordigen_details.product,
                            cash_account_type: nordigen_details.cashAccountType,
                            status: nordigen_details.status,
                            details: nordigen_details.details,
                            owner_name: nordigen_details.ownerName,
                            icon: Some(nordigen_institution.logo),
                            institution_name: Some(nordigen_institution.name),
                            account_type: Some("cash".into()),
                        }.update(&mut con)?;
                        // ));
                    }
                }
                Ok(())
            }
            AccountsCommand::GetNordigenAccountBalances { account_id } => {
                let mut client = Nordigen::new();
                let mut con = establish_connection();
                client.populate_token()?;
                let account: Account = schema::accounts::dsl::accounts
                    .find(account_id)
                    .first(&mut con)?;
                let balances = client.get_account_balances(&account.nordigen_id)?;
                dbg!(balances);
                Ok(())
            },
            AccountsCommand::UpdateBalances {} => {
                let mut client = Nordigen::new();
                client.populate_token()?;
                let mut con = establish_connection();
                use diesel::*;
                let accounts = Account::all().load(&mut con)?;
                for mut account in accounts {
                    account.update_balance()?;
                    account.update(&mut con)?;
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

                let user = schema::users::dsl::users
                    .find(db_requisition.user_id)
                    .first(&mut con)?;

                for account_id in requisition.accounts {
                    let account_details = client.get_account_details(&account_id)?;
                    NewAccount::new("", &account_id, &account_details, &user)?;
                    println!("Account {} added.", &account_id);
                }

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
                // let mut con = establish_connection();
                // use diesel::*;
                // let account = schema::accounts::dsl::accounts
                //     .find(account_id)
                //     .first(&mut con)?;
                // let imported_transactions = ultrafinance::import_transactions(account, &mut con)?;
                // println!("imported {} transactions.", imported_transactions.len());
                Ok(())
            }
            TransactionsCommand::CreateTrigger { id } => {
                let mut con = establish_connection();
                let transaction: Transaction = self::schema::transactions::dsl::transactions
                    .find(id)
                    .first(&mut con)?;

                ultrafinance::create_transaction_trigger_queue(&transaction, &mut con)
            }
        },
        Commands::Server(command) => match command {
            ServerCommand::Start => {
                server::start().map_err(|e|anyhow::anyhow!(e.to_string()))
            },
        },
    }
}
