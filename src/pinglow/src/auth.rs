use std::{collections::HashMap, sync::Arc};

use futures::StreamExt;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use k8s_openapi::{api::core::v1::Secret, ByteString};
use kube::{
    api::{Patch, PatchParams},
    runtime::{
        controller::{Action, Controller},
        finalizer::{finalizer, Event},
        watcher,
    },
    Api, Client, Resource, ResourceExt,
};
use log::warn;
use rand::{distr::Alphanumeric, RngExt};
use redis::{AsyncCommands, Client as RedisClient};
use rocket::{
    get,
    http::{Cookie, CookieJar, SameSite, Status},
    post,
    request::{FromRequest, Outcome},
    response::Redirect,
    Request, State,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::config::{OidcConfig, PinglowConfig};
use pinglow_common::{ApiKeyBinding, PinglowUserBinding, UserRole};

type BindingCache = Arc<RwLock<HashMap<String, HashMap<String, UserRole>>>>;
type ApiKeyCache = Arc<RwLock<HashMap<String, ApiKeyCredential>>>;

#[derive(Clone)]
struct ApiKeyCredential {
    binding: String,
    role: UserRole,
}

#[derive(Clone)]
pub struct AuthState {
    pub redis: RedisClient,
    pub bindings: BindingCache,
    api_keys: ApiKeyCache,
    pub oidc: Option<OidcConfig>,
    pub cookie_secure: bool,
    http_client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionUser {
    pub user: String,
    pub role: UserRole,
}

#[derive(Debug, Deserialize)]
struct Discovery {
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: String,
}

#[derive(Debug, Deserialize)]
struct Claims {
    aud: Value,
    sub: String,
    email: Option<String>,
    nonce: String,
    #[serde(flatten)]
    additional: serde_json::Map<String, Value>,
}

pub async fn create_auth_state(
    config: &PinglowConfig,
    redis: RedisClient,
) -> Result<Arc<AuthState>, Box<dyn std::error::Error + Send + Sync>> {
    let http_client = build_oidc_http_client(config.oidc.as_ref())?;
    let state = Arc::new(AuthState {
        redis,
        bindings: Arc::new(RwLock::new(HashMap::new())),
        api_keys: Arc::new(RwLock::new(HashMap::new())),
        oidc: config.oidc.clone(),
        cookie_secure: config.oidc_cookie_secure,
        http_client,
    });
    refresh_bindings(config, &state).await?;
    Ok(state)
}

/// Builds the HTTP client used for OIDC provider calls. If
/// `oidc.extra_ca_cert_path` is set, the referenced PEM file is added as an
/// additional trusted root on top of the system's default CA bundle -
/// existing public-CA-issued endpoints keep working unchanged.
fn build_oidc_http_client(
    oidc: Option<&OidcConfig>,
) -> Result<reqwest::Client, Box<dyn std::error::Error + Send + Sync>> {
    let mut builder = reqwest::Client::builder();
    if let Some(path) = oidc.and_then(|oidc| oidc.extra_ca_cert_path.as_ref()) {
        let pem = std::fs::read(path)
            .map_err(|e| format!("failed to read OIDC_EXTRA_CA_CERT_PATH '{path}': {e}"))?;
        let cert = reqwest::Certificate::from_pem(&pem)
            .map_err(|e| format!("failed to parse OIDC_EXTRA_CA_CERT_PATH '{path}' as PEM: {e}"))?;
        // Merges with the platform's native/built-in roots rather than
        // replacing them, so public-CA-issued endpoints keep working too.
        builder = builder.tls_certs_merge([cert]);
    }
    Ok(builder.build()?)
}

#[derive(Clone)]
struct ApiKeyContext {
    client: Client,
    state: Arc<AuthState>,
    namespace: String,
}

#[derive(Debug, thiserror::Error)]
enum ApiKeyReconcileError {
    #[error("Kubernetes error: {0}")]
    Kubernetes(#[from] kube::Error),
    #[error("reconciliation error: {0}")]
    Message(String),
    #[error("finalizer error: {0}")]
    Finalizer(String),
}

pub async fn reconcile_api_keys(
    config: PinglowConfig,
    state: Arc<AuthState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::try_default().await?;
    let bindings = Api::<ApiKeyBinding>::namespaced(client.clone(), &config.target_namespace);
    let context = Arc::new(ApiKeyContext {
        client,
        state,
        namespace: config.target_namespace,
    });
    Controller::new(bindings, watcher::Config::default())
        .run(reconcile_api_key, api_key_error_policy, context)
        .for_each(|result| async move {
            if let Err(error) = result {
                warn!("API key reconciliation error: {error}");
            }
        })
        .await;
    Ok(())
}

async fn reconcile_api_key(
    binding: Arc<ApiKeyBinding>,
    context: Arc<ApiKeyContext>,
) -> Result<Action, ApiKeyReconcileError> {
    let binding_api = Api::<ApiKeyBinding>::namespaced(context.client.clone(), &context.namespace);
    let binding_name = binding.name_any();
    let secret_name = binding
        .spec
        .secret_name
        .clone()
        .unwrap_or_else(|| format!("{binding_name}-api-key"));
    finalizer(
        &binding_api,
        "pinglow.io/api-key-finalizer",
        binding,
        |event| {
            let context = context.clone();
            async move {
                match event {
                    Event::Apply(binding) => {
                        let secrets =
                            Api::<Secret>::namespaced(context.client.clone(), &context.namespace);
                        let secret = match secrets.get_opt(&secret_name).await? {
                            Some(secret) => secret,
                            None => {
                                let key = random_value();
                                let owner = binding.controller_owner_ref(&()).ok_or_else(|| {
                                    ApiKeyReconcileError::Message("missing owner reference".into())
                                })?;
                                let secret = Secret {
                                    metadata: kube::api::ObjectMeta {
                                        name: Some(secret_name.clone()),
                                        namespace: Some(context.namespace.clone()),
                                        owner_references: Some(vec![owner]),
                                        ..Default::default()
                                    },
                                    type_: Some("Opaque".into()),
                                    data: Some(
                                        [(
                                            String::from("API_KEY"),
                                            ByteString(key.as_bytes().to_vec()),
                                        )]
                                        .into_iter()
                                        .collect(),
                                    ),
                                    ..Default::default()
                                };
                                secrets
                                    .patch(
                                        &secret_name,
                                        &PatchParams::apply("pinglow"),
                                        &Patch::Apply(secret),
                                    )
                                    .await?
                            }
                        };
                        context
                            .state
                            .api_keys
                            .write()
                            .await
                            .retain(|_, credential| credential.binding != binding_name);
                        if let Some(data) =
                            secret.data.and_then(|data| data.get("API_KEY").cloned())
                        {
                            let key = String::from_utf8_lossy(&data.0).to_string();
                            context.state.api_keys.write().await.insert(
                                key,
                                ApiKeyCredential {
                                    binding: binding_name.clone(),
                                    role: binding.spec.role.clone(),
                                },
                            );
                        }
                        Ok(Action::await_change())
                    }
                    Event::Cleanup(_) => {
                        let secrets =
                            Api::<Secret>::namespaced(context.client.clone(), &context.namespace);
                        let _ = secrets.delete(&secret_name, &Default::default()).await;
                        context
                            .state
                            .api_keys
                            .write()
                            .await
                            .retain(|_, credential| credential.binding != binding_name);
                        Ok(Action::await_change())
                    }
                }
            }
        },
    )
    .await
    .map_err(
        |error: kube::runtime::finalizer::Error<ApiKeyReconcileError>| {
            ApiKeyReconcileError::Finalizer(error.to_string())
        },
    )
}

fn api_key_error_policy(
    _: Arc<ApiKeyBinding>,
    _: &ApiKeyReconcileError,
    _: Arc<ApiKeyContext>,
) -> Action {
    Action::requeue(std::time::Duration::from_secs(30))
}

pub async fn watch_bindings(
    config: PinglowConfig,
    state: Arc<AuthState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::try_default().await?;
    let api: Api<PinglowUserBinding> = Api::namespaced(client, &config.target_namespace);
    watcher(api, watcher::Config::default())
        .for_each(|event| {
            let state = state.clone();
            async move {
                match event {
                    Ok(watcher::Event::Apply(binding) | watcher::Event::InitApply(binding)) => {
                        let mut cache = state.bindings.write().await;
                        replace_binding(&mut cache, &binding);
                    }
                    Ok(watcher::Event::Delete(binding)) => {
                        let mut cache = state.bindings.write().await;
                        remove_binding(&mut cache, &binding.name_any());
                    }
                    Ok(watcher::Event::Init) => state.bindings.write().await.clear(),
                    Ok(watcher::Event::InitDone) => {}
                    Err(error) => warn!("User binding watch error: {error}"),
                }
            }
        })
        .await;
    Ok(())
}

async fn refresh_bindings(config: &PinglowConfig, state: &AuthState) -> Result<(), kube::Error> {
    let client = Client::try_default().await?;
    let api: Api<PinglowUserBinding> = Api::namespaced(client, &config.target_namespace);
    let bindings = api.list(&Default::default()).await?;
    let mut cache = state.bindings.write().await;
    cache.clear();
    for binding in bindings {
        insert_binding(&mut cache, &binding);
    }
    Ok(())
}

fn replace_binding(
    cache: &mut HashMap<String, HashMap<String, UserRole>>,
    binding: &PinglowUserBinding,
) {
    let name = binding.name_any();
    remove_binding(cache, &name);
    insert_binding(cache, binding);
}

fn remove_binding(cache: &mut HashMap<String, HashMap<String, UserRole>>, name: &str) {
    for entries in cache.values_mut() {
        entries.remove(name);
    }
    cache.retain(|_, entries| !entries.is_empty());
}

fn insert_binding(
    cache: &mut HashMap<String, HashMap<String, UserRole>>,
    binding: &PinglowUserBinding,
) {
    let name = binding.name_any();
    for identity in [binding.spec.subject.clone(), binding.spec.email.clone()]
        .into_iter()
        .flatten()
        .chain(binding.spec.groups.clone().unwrap_or_default())
    {
        cache
            .entry(identity)
            .or_default()
            .insert(name.clone(), binding.spec.role.clone());
    }
}

fn role_priority(role: &UserRole) -> u8 {
    match role {
        UserRole::Viewer => 0,
        UserRole::Operator => 1,
        UserRole::Admin => 2,
    }
}

fn group_claims(claims: &Claims, claim_path: &str) -> Vec<String> {
    let root = Value::Object(claims.additional.clone());
    let mut value = &root;
    for segment in claim_path.split('.') {
        let Some(next) = value.get(segment) else {
            return Vec::new();
        };
        value = next;
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn resolve_role(
    bindings: &HashMap<String, HashMap<String, UserRole>>,
    claims: &Claims,
    groups_claim: &str,
) -> Option<UserRole> {
    let groups = group_claims(claims, groups_claim);
    std::iter::once(claims.sub.as_str())
        .chain(claims.email.as_deref())
        .chain(groups.iter().map(String::as_str))
        .filter_map(|claim| bindings.get(claim))
        .flat_map(|binding_roles| binding_roles.values())
        .cloned()
        .max_by_key(role_priority)
}

fn random_value() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(48)
        .map(char::from)
        .collect()
}

fn session_cookie(state: &AuthState, value: String) -> Cookie<'static> {
    Cookie::build(("pinglow_session", value))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.cookie_secure)
        .path("/")
        .build()
}

async fn redis_get(state: &AuthState, key: &str) -> Option<SessionUser> {
    let mut connection = state.redis.get_multiplexed_async_connection().await.ok()?;
    let value: Option<String> = connection.get(key).await.ok()?;
    serde_json::from_str(&value?).ok()
}

pub struct Authenticated(pub SessionUser);
pub struct Operator(pub SessionUser);
pub struct OperatorApiKey;

async fn api_key_role(request: &Request<'_>) -> Outcome<UserRole, ()> {
    let state = match request.rocket().state::<Arc<AuthState>>() {
        Some(state) => state,
        None => return Outcome::Error((Status::ServiceUnavailable, ())),
    };
    let keys: Vec<_> = request.headers().get("x-api-key").collect();
    if keys.len() != 1 {
        return Outcome::Error((Status::Unauthorized, ()));
    }
    match state
        .api_keys
        .read()
        .await
        .get(keys[0])
        .map(|credential| credential.role.clone())
    {
        Some(role) => Outcome::Success(role),
        None => Outcome::Error((Status::Unauthorized, ())),
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for OperatorApiKey {
    type Error = ();
    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        match api_key_role(request).await {
            Outcome::Success(UserRole::Operator | UserRole::Admin) => {
                Outcome::Success(OperatorApiKey)
            }
            Outcome::Success(_) => Outcome::Error((Status::Forbidden, ())),
            Outcome::Error(error) => Outcome::Error(error),
            Outcome::Forward(status) => Outcome::Forward(status),
        }
    }
}

async fn authenticated(request: &Request<'_>) -> Outcome<SessionUser, ()> {
    let state = match request.rocket().state::<Arc<AuthState>>() {
        Some(state) => state,
        None => return Outcome::Error((Status::ServiceUnavailable, ())),
    };
    if let Some(cookie) = request.cookies().get("pinglow_session") {
        if let Some(user) = redis_get(state, &format!("pinglow:session:{}", cookie.value())).await {
            return Outcome::Success(user);
        }
    }
    let keys: Vec<_> = request.headers().get("x-api-key").collect();
    if keys.len() == 1 {
        if let Some(credential) = state.api_keys.read().await.get(keys[0]) {
            return Outcome::Success(SessionUser {
                user: credential.binding.clone(),
                role: credential.role.clone(),
            });
        }
    }
    Outcome::Error((Status::Unauthorized, ()))
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for Authenticated {
    type Error = ();
    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        authenticated(request).await.map(Authenticated)
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for Operator {
    type Error = ();
    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        match authenticated(request).await {
            Outcome::Success(user) if matches!(user.role, UserRole::Operator | UserRole::Admin) => {
                Outcome::Success(Operator(user))
            }
            Outcome::Success(_) => Outcome::Error((Status::Forbidden, ())),
            Outcome::Error(error) => Outcome::Error(error),
            Outcome::Forward(status) => Outcome::Forward(status),
        }
    }
}

#[get("/auth/login")]
pub async fn login(
    state: &State<Arc<AuthState>>,
    cookies: &CookieJar<'_>,
) -> Result<Redirect, Status> {
    let oidc = state.oidc.as_ref().ok_or(Status::NotFound)?;
    let discovery = discover(state.inner())
        .await
        .map_err(|_| Status::ServiceUnavailable)?;
    let oauth_state = random_value();
    let nonce = random_value();
    let mut connection = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| Status::ServiceUnavailable)?;
    let _: () = redis::cmd("SETEX")
        .arg(format!("pinglow:oidc:{}", oauth_state))
        .arg(300)
        .arg(&nonce)
        .query_async(&mut connection)
        .await
        .map_err(|_| Status::ServiceUnavailable)?;
    let url = format!("{}?response_type=code&client_id={}&redirect_uri={}&scope=openid%20profile%20email&state={}&nonce={}", discovery.authorization_endpoint, urlencoding::encode(&oidc.client_id), urlencoding::encode(&oidc.redirect_url), oauth_state, nonce);
    cookies.add(session_cookie(state.inner(), oauth_state));
    Ok(Redirect::to(url))
}

#[get("/auth/callback?<code>&<state>")]
pub async fn callback(
    auth_state: &State<Arc<AuthState>>,
    cookies: &CookieJar<'_>,
    code: String,
    state: String,
) -> Result<Redirect, Status> {
    let oidc = auth_state.oidc.as_ref().ok_or(Status::NotFound)?;
    let login_cookie = cookies.get("pinglow_session").ok_or(Status::Unauthorized)?;
    if login_cookie.value() != state {
        return Err(Status::Unauthorized);
    }
    let mut connection = auth_state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| Status::ServiceUnavailable)?;
    let nonce: Option<String> = connection
        .get_del(format!("pinglow:oidc:{}", state))
        .await
        .map_err(|_| Status::Unauthorized)?;
    let nonce = nonce.ok_or(Status::Unauthorized)?;
    let discovery = discover(auth_state.inner())
        .await
        .map_err(|_| Status::ServiceUnavailable)?;
    let token: TokenResponse = auth_state
        .http_client
        .post(&discovery.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", oidc.redirect_url.as_str()),
            ("client_id", oidc.client_id.as_str()),
            ("client_secret", oidc.client_secret.as_str()),
        ])
        .send()
        .await
        .map_err(|_| Status::Unauthorized)?
        .error_for_status()
        .map_err(|_| Status::Unauthorized)?
        .json()
        .await
        .map_err(|_| Status::Unauthorized)?;
    let claims = verify_id_token(auth_state.inner(), &discovery, &token.id_token, &nonce)
        .await
        .map_err(|_| Status::Unauthorized)?;
    let role = {
        let bindings = auth_state.bindings.read().await;
        resolve_role(&bindings, &claims, &oidc.groups_claim)
    }
    .ok_or(Status::Forbidden)?;
    let session_id = random_value();
    let user = SessionUser {
        user: claims.email.unwrap_or(claims.sub),
        role,
    };
    let _: () = redis::cmd("SETEX")
        .arg(format!("pinglow:session:{}", session_id))
        .arg(28800)
        .arg(serde_json::to_string(&user).map_err(|_| Status::InternalServerError)?)
        .query_async(&mut connection)
        .await
        .map_err(|_| Status::ServiceUnavailable)?;
    cookies.remove(Cookie::build("pinglow_session").path("/").build());
    cookies.add(session_cookie(auth_state.inner(), session_id));
    Ok(Redirect::to("/"))
}

#[get("/auth/me")]
pub async fn me(user: Authenticated) -> rocket::serde::json::Json<SessionUser> {
    rocket::serde::json::Json(user.0)
}

#[post("/auth/logout")]
pub async fn logout(state: &State<Arc<AuthState>>, cookies: &CookieJar<'_>) -> Status {
    if let Some(cookie) = cookies.get("pinglow_session") {
        if let Ok(mut connection) = state.redis.get_multiplexed_async_connection().await {
            let _: Result<(), _> = connection
                .del(format!("pinglow:session:{}", cookie.value()))
                .await;
        }
    }
    cookies.remove(Cookie::build("pinglow_session").path("/").build());
    Status::NoContent
}

async fn discover(
    state: &AuthState,
) -> Result<Discovery, Box<dyn std::error::Error + Send + Sync>> {
    let oidc = state.oidc.as_ref().ok_or("OIDC is disabled")?;
    Ok(state
        .http_client
        .get(format!(
            "{}/.well-known/openid-configuration",
            oidc.issuer.trim_end_matches('/')
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn verify_id_token(
    state: &AuthState,
    discovery: &Discovery,
    token: &str,
    nonce: &str,
) -> Result<Claims, Box<dyn std::error::Error + Send + Sync>> {
    let header = decode_header(token)?;
    let key_set: Value = state
        .http_client
        .get(&discovery.jwks_uri)
        .send()
        .await?
        .json()
        .await?;
    let key = key_set["keys"]
        .as_array()
        .and_then(|keys| {
            keys.iter()
                .find(|key| key["kid"].as_str() == header.kid.as_deref())
        })
        .ok_or("signing key not found")?;
    let decoding_key = DecodingKey::from_rsa_components(
        key["n"].as_str().ok_or("missing modulus")?,
        key["e"].as_str().ok_or("missing exponent")?,
    )?;
    let mut validation = Validation::new(Algorithm::RS256);
    let oidc = state.oidc.as_ref().ok_or("OIDC is disabled")?;
    validation.set_issuer(&[oidc.issuer.as_str()]);
    validation.set_audience(&[oidc.client_id.as_str()]);
    let claims = decode::<Claims>(token, &decoding_key, &validation)?.claims;
    if claims.nonce != nonce || !claims.aud.to_string().contains(&oidc.client_id) {
        return Err("invalid nonce or audience".into());
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_claims(additional: Value) -> Claims {
        Claims {
            aud: Value::String("client".to_string()),
            sub: "user-123".to_string(),
            email: Some("alice@example.com".to_string()),
            nonce: "nonce".to_string(),
            additional: additional.as_object().unwrap().clone(),
        }
    }

    #[test]
    fn test_group_claims_support_nested_paths() {
        let claims = test_claims(serde_json::json!({
            "realm_access": { "roles": ["operators", "employees"] }
        }));

        assert_eq!(
            group_claims(&claims, "realm_access.roles"),
            vec!["operators", "employees"]
        );
    }

    #[test]
    fn test_group_claims_ignore_missing_or_non_string_values() {
        let claims = test_claims(serde_json::json!({
            "groups": ["operators", 42]
        }));

        assert_eq!(group_claims(&claims, "groups"), vec!["operators"]);
        assert!(group_claims(&claims, "missing").is_empty());
    }

    #[test]
    fn test_resolve_role_uses_most_privileged_matching_binding() {
        let claims = test_claims(serde_json::json!({
            "groups": ["operators", "admins"]
        }));
        let bindings = HashMap::from([
            (
                "user-123".to_string(),
                HashMap::from([("user-binding".to_string(), UserRole::Viewer)]),
            ),
            (
                "operators".to_string(),
                HashMap::from([("operator-binding".to_string(), UserRole::Operator)]),
            ),
            (
                "admins".to_string(),
                HashMap::from([("admin-binding".to_string(), UserRole::Admin)]),
            ),
        ]);

        assert_eq!(
            resolve_role(&bindings, &claims, "groups"),
            Some(UserRole::Admin)
        );
    }

    #[test]
    fn test_random_value_length() {
        let value = random_value();
        assert_eq!(
            value.len(),
            48,
            "random_value should generate 48 characters"
        );
    }

    #[test]
    fn test_random_value_uniqueness() {
        let value1 = random_value();
        let value2 = random_value();
        assert_ne!(
            value1, value2,
            "random_value should generate different values on each call"
        );
    }

    #[test]
    fn test_random_value_alphanumeric() {
        let value = random_value();
        assert!(
            value.chars().all(|c| c.is_alphanumeric()),
            "random_value should contain only alphanumeric characters"
        );
    }

    #[test]
    fn test_session_user_serialization() {
        let user = SessionUser {
            user: "alice@example.com".to_string(),
            role: UserRole::Admin,
        };
        let json = serde_json::to_string(&user).expect("serialization failed");
        let deserialized: SessionUser =
            serde_json::from_str(&json).expect("deserialization failed");
        assert_eq!(deserialized.user, user.user);
        assert_eq!(deserialized.role, user.role);
    }

    #[test]
    fn test_api_key_credential_creation() {
        let credential = ApiKeyCredential {
            binding: "test-binding".to_string(),
            role: UserRole::Operator,
        };
        assert_eq!(credential.binding, "test-binding");
        assert_eq!(credential.role, UserRole::Operator);
    }

    #[test]
    fn test_user_role_equality() {
        assert_eq!(UserRole::Admin, UserRole::Admin);
        assert_ne!(UserRole::Admin, UserRole::Operator);
        assert_ne!(UserRole::Operator, UserRole::Viewer);
    }

    #[test]
    fn test_session_user_role_matching() {
        let admin_user = SessionUser {
            user: "admin@test.com".to_string(),
            role: UserRole::Admin,
        };
        let operator_user = SessionUser {
            user: "op@test.com".to_string(),
            role: UserRole::Operator,
        };
        let viewer_user = SessionUser {
            user: "view@test.com".to_string(),
            role: UserRole::Viewer,
        };

        assert!(matches!(admin_user.role, UserRole::Admin));
        assert!(matches!(operator_user.role, UserRole::Operator));
        assert!(matches!(viewer_user.role, UserRole::Viewer));
    }
}
