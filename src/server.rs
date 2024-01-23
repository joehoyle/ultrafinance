use std::{fmt::Display, pin::Pin, path::PathBuf, str::FromStr};

use actix_files::NamedFile;
use actix_web::{web::{Data, block}, App, FromRequest, HttpRequest, HttpServer, dev::Service, http::header};

use actix_cors::Cors;

use actix_identity::{Identity, IdentityMiddleware, IdentityExt};
use actix_session::{storage::CookieSessionStore, SessionMiddleware};

use futures::FutureExt;
use paperclip::actix::{
    api_v2_errors,
    web::{self},
    OpenApiExt,
};

use diesel::mysql::MysqlConnection;
use dotenvy::dotenv;
use serde::Serialize;

use std::env;
use crate::ultrafinance::{hash_api_key, is_dev, DbPool};
use crate::schema;
use crate::endpoints;

use crate::models::*;

pub struct AppState {
    pub db: diesel::r2d2::Pool<diesel::r2d2::ConnectionManager<diesel::mysql::MysqlConnection>>,
    pub url: String,
}

impl FromRequest for User {
    type Error = actix_web::Error;
    type Future = Pin<Box<dyn futures::Future<Output = Result<User, actix_web::Error>>>>;

    fn from_request(req: &HttpRequest, _pl: &mut actix_web::dev::Payload) -> Self::Future {
        let db = req.app_data::<Data<AppState>>().unwrap().db.clone();

        let id = Identity::from_request(req, _pl).into_inner();
        match id {
            Ok(id) => {
                return Box::pin(async move {
                    let id = id.id();
                    let result = match id {
                        Ok(user_id) => {
                            let user = match user_id.parse::<i32>() {
                                Ok(user_id) => {
                                    let user = block(move || -> Result<User, Error> {
                                        use diesel::*;
                                        let con = db.get();
                                        User::by_id(user_id).first(&mut con.unwrap()).map_err(|e|e.into())
                                    }).await.unwrap();
                                    user
                                },
                                Err(_) => {
                                    Err(anyhow::anyhow!("Unable to parse id").into())
                                },
                            };
                            user
                        },
                        Err(_) => Err(anyhow::anyhow!("Unable to get id").into()),
                    };
                    match result {
                        Ok(user) => Ok(user),
                        Err(e) => Err(actix_web::error::ErrorUnauthorized(e.to_string())),
                    }
                });
            }
            Err(_) => {

            }
        }

        let authorization_header = req.headers().get("Authorization");
        if authorization_header.is_none() {
            return Box::pin(async { Err(actix_web::error::ErrorUnauthorized("unauthorized")) });
        }
        let mut parts = authorization_header
            .unwrap()
            .to_str()
            .unwrap()
            .splitn(2, ' ');

        match parts.next() {
            Some(scheme) if scheme == "Bearer" => {}
            _ => {
                return Box::pin(async {
                    Err(actix_web::error::ErrorUnauthorized(
                        "Missing scheme in Authorization header.",
                    ))
                })
            }
        }

        let token = match parts.next() {
            Some(token) => token,
            _ => {
                return Box::pin(async {
                    Err(actix_web::error::ErrorUnauthorized(
                        "Missing scheme in Authorization header.",
                    ))
                })
            }
        }
        .to_owned();

        Box::pin(async move {
            let user = get_user_by_api_key(token, &db).await;
            match user {
                Ok(user) => Ok(user),
                Err(e) => Err(actix_web::error::ErrorUnauthorized(e.to_string())),
            }
        })
    }
}

#[derive(Debug, Serialize)]
#[api_v2_errors(
    code = 400,
    code = 401,
    description = "Unauthorized: Can't read session from header",
    code = 500
)]
pub struct Error {
    #[serde(skip_serializing)]
    err: anyhow::Error,
}

impl actix_web::error::ResponseError for Error {
    fn status_code(&self) -> reqwest::StatusCode {
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    }
}
impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Error {
        Error { err }
    }
}

impl From<r2d2::Error> for Error {
    fn from(err: r2d2::Error) -> Error {
        Error {
            err: anyhow::anyhow!(err.to_string()),
        }
    }
}

impl From<diesel::result::Error> for Error {
    fn from(err: diesel::result::Error) -> Error {
        Error {
            err: anyhow::anyhow!(err.to_string()),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Error {
        Error {
            err: anyhow::anyhow!(err.to_string()),
        }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.err.to_string())
    }
}

async fn index(_req: HttpRequest) -> actix_web::Result<NamedFile> {
    let path: PathBuf = "./frontend/dist/index.html".parse().unwrap();
    Ok(NamedFile::open(path)?)
}

#[actix_web::main]
pub async fn start() -> std::io::Result<()> {
    dotenv().ok();
    let manager = diesel::r2d2::ConnectionManager::<MysqlConnection>::new(
        env::var("DATABASE_URL").unwrap().as_str(),
    );
    let pool = diesel::r2d2::Pool::builder().build(manager).unwrap();
    println!("Listening on http://0.0.0.0:3000");
    // let secret_key = actix_web::cookie::Key::generate();
    // let output = &secret_key.master();
    // let secret: String = base64::Engine::encode(&base64::engine::general_purpose::STANDARD_NO_PAD, *output);

    let secret_key = env::var("COOKIE_SECRET").unwrap();
    let secret_key = base64::Engine::decode(&base64::engine::general_purpose::STANDARD_NO_PAD, secret_key).unwrap();
    let secret_key = actix_web::cookie::Key::from(secret_key.as_slice());

    HttpServer::new(move || {
        let session_mw = SessionMiddleware::builder(CookieSessionStore::default(), secret_key.clone())
            // disable secure cookie for local testing
            .cookie_secure(!is_dev())
            .cookie_same_site( match is_dev() {
                true => actix_web::cookie::SameSite::None,
                false => actix_web::cookie::SameSite::Strict,
            } )
            .build();

        let cors = Cors::default()
            .allowed_origin(env::var("SITE_URL").unwrap().as_str())
            .allow_any_header()
            .allow_any_method()
            .supports_credentials()
            .max_age(3600);
         App::new()
            .service(
                actix_files::Files::new("/frontend/dist/", "./frontend/dist/").show_files_listing(),
            )
            .wrap_fn(|req, srv| {
                let identity = req.get_identity();
                let has_authorization_header = req.headers().get("Authorization").is_some();
                srv.call(req).map(move |res| {
                    let res = res.map(|mut response| {
                        let build_id = option_env!("BUILD_ID").unwrap_or("dev");
                        response.headers_mut().insert(header::HeaderName::from_str("X-Build").unwrap(), header::HeaderValue::from_str(build_id).unwrap());
                        if identity.is_ok() || has_authorization_header {
                            response.headers_mut().insert(header::HeaderName::from_str("Cache-Control").unwrap(), header::HeaderValue::from_str("private, no-cache, must-revalidate").unwrap());
                        }
                        response
                    } );
                    res
                })
            })
            .route("/", actix_web::web::get().to(index))
            .wrap(IdentityMiddleware::default())
            .wrap(session_mw)
            .wrap(cors)
            .wrap_api()
            .with_json_spec_at("/api/spec/v2")
            .with_swagger_ui_at("/docs")
            .app_data(Data::new(AppState {
                db: pool.clone(),
                url: env::var("SITE_URL").unwrap().to_owned(),
            }))
            .service(
                web::scope("/api/v1")
                    .service(
                        web::resource("/accounts")
                            .route(web::get().to(endpoints::accounts::get_accounts_endpoint))
                            .route(web::post().to(endpoints::accounts::create_accounts_endpoint)),
                    )
                    .service(
                        web::resource("/accounts/sync")
                            .route(web::post().to(endpoints::accounts::sync_accounts_endpoint)),
                    )
                    .service(
                        web::resource("/accounts/{account_id}")
                            .route(web::get().to(endpoints::accounts::get_account_endpoint))
                            .route(web::post().to(endpoints::accounts::update_account_endpoint))
                            .route(web::delete().to(endpoints::accounts::delete_account_endpoint)),
                    )
                    .service(
                        web::resource("/accounts/{account_id}/relink")
                            .route(web::post().to(endpoints::accounts::relink_account_endpoint)),
                    )
                    .service(
                        web::resource("/accounts/{account_id}/sync")
                            .route(web::post().to(endpoints::accounts::sync_account_endpoint)),
                    )
                    .service(
                        web::resource("/functions")
                            .route(web::get().to(endpoints::functions::get_functions_endpoint))
                            .route(web::post().to(endpoints::functions::create_function_endpoint)),
                    )
                    .service(
                        web::resource("/functions/{function_id}")
                            .route(web::get().to(endpoints::functions::get_function_endpoint))
                            .route(web::post().to(endpoints::functions::update_function_endpoint))
                            .route(web::delete().to(endpoints::functions::delete_function_endpoint)),
                    )
                    .service(
                        web::resource("/functions/{function_id}/test")
                            .route(web::post().to(endpoints::functions::test_function_endpoint))
                    )
                    .service(
                        web::resource("/requisitions")
                            .route(web::post().to(endpoints::requisitions::create_requisition_endpoint)),
                    )
                    .service(web::resource("/requisitions/institutions").route(
                        web::get().to(endpoints::requisitions::get_requisitions_institutions_endpoint),
                    ))
                    .service(
                        web::resource("/transactions")
                            .route(web::get().to(endpoints::transactions::get_transactions_endpoint)),
                    )
                    .service(
                        web::resource("/transactions/{transaction_id}")
                            .route(web::get().to(endpoints::transactions::get_transaction_endpoint))
                            .route(web::delete().to(endpoints::transactions::delete_transaction_endpoint)),
                    )
                    .service(
                        web::resource("/triggers")
                            .route(web::get().to(endpoints::triggers::get_triggers_endpoint))
                            .route(web::post().to(endpoints::triggers::create_trigger_endpoint)),
                    )
                    .service(
                        web::resource("/triggers/queue")
                            .route(web::get().to(endpoints::triggers::get_trigger_queue_endpoint)),
                    )
                    .service(
                        web::resource("/triggers/queue/process")
                            .route(web::post().to(endpoints::triggers::process_trigger_queue_endpoint)),
                    )
                    .service(
                        web::resource("/triggers/log")
                            .route(web::get().to(endpoints::triggers::get_trigger_log_endpoint)),
                    )
                    .service(
                        web::resource("/triggers/{trigger_id}")
                            .route(web::get().to(endpoints::triggers::get_trigger_endpoint))
                            .route(web::delete().to(endpoints::triggers::delete_trigger_endpoint))
                            .route(web::post().to(endpoints::triggers::update_trigger_endpoint)),
                    )
                    .service(
                        web::resource("/users/me")
                            .route(web::get().to(endpoints::users::get_me_endpoint))
                            .route(web::post().to(endpoints::users::update_me_endpoint)),
                    )
                    .service(
                        web::resource("/users/session")
                            .route(web::post().to(endpoints::users::create_session_endpoint))
                            .route(web::delete().to(endpoints::users::delete_session_endpoint)),
                    )
                    .service(
                        web::resource("/users")
                            .route(web::post().to(endpoints::users::create_user_endpoint)),
                    )
            )
            .build()
            .route("/", actix_web::web::get().to(index))
            .route("/login", actix_web::web::get().to(index))
            .route("/signup", actix_web::web::get().to(index))
            .route("/accounts", actix_web::web::get().to(index))
            .route("/accounts/new", actix_web::web::get().to(index))
            .route("/accounts/resume", actix_web::web::get().to(index))
            .route("/accounts/{account_id}", actix_web::web::get().to(index))
            .route("/transactions", actix_web::web::get().to(index))
            .route("/transactions/{transaction_id}", actix_web::web::get().to(index))
            .route("/functions", actix_web::web::get().to(index))
            .route("/functions/new", actix_web::web::get().to(index))
            .route("/functions/{function_id}", actix_web::web::get().to(index))
            .route("/triggers", actix_web::web::get().to(index))
            .route("/triggers/new", actix_web::web::get().to(index))
            .route("/triggers/{function_id}", actix_web::web::get().to(index))
            .route("/logs", actix_web::web::get().to(index))
            .route("/account", actix_web::web::get().to(index))
    })
    .bind(("0.0.0.0", 3000))?
    .run()
    .await
}

pub async fn get_user_by_api_key(
    raw_api_key: String,
    db_pool: &DbPool,
) -> Result<User, anyhow::Error> {
    let mut db = db_pool.get().map_err(anyhow::Error::msg).unwrap();

    actix_web::web::block(move || {
        let hashed = hash_api_key(raw_api_key.as_str());
        use diesel::*;
        use schema::user_api_keys::dsl::*;
        let query = user_api_keys.select(user_id).filter(api_key.eq(hashed));
        let found_user_id: i32 = query.first(&mut db)?;
        let user: User = schema::users::dsl::users
            .find(found_user_id)
            .first(&mut db)?;
        Ok(user)
    })
    .await?
}
