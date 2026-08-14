use actix_cors::Cors;
use actix_files;
use actix_web::rt;
use actix_web::{middleware, web, App, HttpRequest, HttpResponse, HttpServer};
use futures::future;
use log::{info, warn, error, debug};
use reqwest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;

fn data_dir() -> PathBuf {
    let dir = env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    PathBuf::from(dir)
}

async fn no_cache<B: actix_web::body::MessageBody>(
    req: actix_web::dev::ServiceRequest,
    next: middleware::Next<B>,
) -> Result<actix_web::dev::ServiceResponse<B>, actix_web::Error> {
    let mut res = next.call(req).await?;
    res.headers_mut().insert(
        actix_web::http::header::CACHE_CONTROL,
        actix_web::http::header::HeaderValue::from_static(
            "no-store, no-cache, must-revalidate",
        ),
    );
    Ok(res)
}

fn endpoints_path() -> PathBuf {
    data_dir().join("endpoints.json")
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct WebhookEvent {
    #[serde(flatten)]
    payload: serde_json::Value,
    #[serde(default)]
    headers: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct WebhookEndpoint {
    #[serde(default)]
    id: String,
    url: String,
    name: String,
    #[serde(default)]
    is_active: bool,
    #[serde(default)]
    source: Option<String>,
}

// Add this new struct for the registration request
#[derive(Debug, Serialize, Deserialize)]
struct CreateWebhookRequest {
    url: String,
    name: String,
    #[serde(default)]
    is_active: bool,
}

struct AppState {
    endpoints: RwLock<Vec<WebhookEndpoint>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct EndpointUpdate {
    is_active: bool,
}

// Endpoint to register new webhook endpoint
async fn register_endpoint(
    endpoint_req: web::Json<CreateWebhookRequest>,
    data: web::Data<AppState>,
) -> HttpResponse {
    info!("POST /endpoints - Register endpoint request: name={}, url={}, is_active={}", endpoint_req.name, endpoint_req.url, endpoint_req.is_active);
    debug!("Register endpoint payload: {:?}", endpoint_req);

    if let Err(e) = url::Url::parse(&endpoint_req.url) {
        warn!("Invalid URL format: {} - {}", endpoint_req.url, e);
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Invalid URL format",
            "details": e.to_string()
        }));
    }

    if endpoint_req.name.trim().is_empty() {
        warn!("Empty name provided for endpoint registration");
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Name cannot be empty"
        }));
    }

    let source = infer_source(&endpoint_req.url, &endpoint_req.name);

    let new_endpoint = WebhookEndpoint {
        id: uuid::Uuid::new_v4().to_string(),
        url: endpoint_req.url.clone(),
        name: endpoint_req.name.clone(),
        is_active: endpoint_req.is_active,
        source,
    };

    let mut endpoints = data.endpoints.write().unwrap();
    endpoints.push(new_endpoint.clone());
    info!("Registered new endpoint: id={}, name={}, url={}", new_endpoint.id, new_endpoint.name, new_endpoint.url);

    if let Err(e) = save_endpoints(&endpoints) {
        error!("Error saving endpoints: {}", e);
    }

    HttpResponse::Ok().json(endpoints.clone())
}

// Endpoint to list all registered webhooks
async fn list_endpoints(data: web::Data<AppState>) -> HttpResponse {
    let endpoints = data.endpoints.read().unwrap();
    info!("GET /endpoints - Listing {} endpoints", endpoints.len());
    debug!("Endpoints: {:?}", endpoints);
    HttpResponse::Ok().json(endpoints.clone())
}

// Update endpoint status (active/inactive)
async fn update_endpoint(
    path: web::Path<String>,
    update: web::Json<EndpointUpdate>,
    data: web::Data<AppState>,
) -> HttpResponse {
    let id = path.into_inner();
    info!("PUT /endpoints/{}/status - Update endpoint request: is_active={}", id, update.is_active);
    debug!("Update payload: {:?}", update);

    let mut endpoints = data.endpoints.write().unwrap();

    if let Some(endpoint) = endpoints.iter_mut().find(|e| e.id == id) {
        endpoint.is_active = update.is_active;
        info!("Updated endpoint: id={}, name={}, is_active={}", endpoint.id, endpoint.name, endpoint.is_active);

        let endpoint_clone = endpoint.clone();

        if let Err(e) = save_endpoints(&endpoints) {
            error!("Error saving endpoints: {}", e);
        }

        HttpResponse::Ok().json(endpoint_clone)
    } else {
        warn!("Endpoint not found for update: id={}", id);
        HttpResponse::NotFound().finish()
    }
}

// Endpoint to delete a webhook endpoint
async fn delete_endpoint(
    endpoint_id: web::Path<String>,
    data: web::Data<AppState>,
) -> HttpResponse {
    let id = endpoint_id.into_inner();
    info!("DELETE /endpoints/{} - Delete endpoint request", id);

    let mut endpoints = data.endpoints.write().unwrap();
    if let Some(pos) = endpoints.iter().position(|e| e.id == id) {
        let removed = endpoints.remove(pos);
        info!("Deleted endpoint: id={}, name={}", removed.id, removed.name);

        if let Err(e) = save_endpoints(&endpoints) {
            error!("Error saving endpoints: {}", e);
        }

        HttpResponse::Ok().json(endpoints.clone())
    } else {
        warn!("Endpoint not found for deletion: id={}", id);
        HttpResponse::NotFound().finish()
    }
}

// Forward webhook to specific endpoint
async fn forward_webhook(
    client: &reqwest::Client,
    endpoint: &WebhookEndpoint,
    payload: &WebhookEvent,
) -> Result<(), String> {
    info!("Forwarding webhook to endpoint: name={}, url={}", endpoint.name, endpoint.url);
    debug!("Forward payload: {:?}", payload);

    let mut request_builder = client.post(&endpoint.url).json(&payload.payload);

    let url = url::Url::parse(&endpoint.url).map_err(|e| format!("Failed to parse URL: {}", e))?;

    let host = url
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;

    let host_header = if let Some(port) = url.port() {
        format!("{}:{}", host, port)
    } else {
        host.to_string()
    };

    request_builder = request_builder.header("Host", host_header);

    for (header_name, header_value) in &payload.headers {
        if header_name.to_lowercase() != "host" {
            request_builder = request_builder.header(header_name, header_value);
        }
    }

    let response = request_builder
        .send()
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;

    let status = response.status();
    if status.is_success() {
        info!("Successfully forwarded to {}: status {}", endpoint.name, status);
        Ok(())
    } else {
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unable to read error response".to_string());
        error!("Endpoint {} returned error status {}: {}", endpoint.name, status, error_body);
        Err(format!(
            "Endpoint returned error status {}: {}",
            status, error_body
        ))
    }
}

// Endpoint to handle source-specific webhooks
async fn handle_specific_webhook(
    path: web::Path<String>,
    payload: web::Json<serde_json::Value>,
    req: HttpRequest,
    data: web::Data<AppState>,
) -> HttpResponse {
    let service = path.into_inner();
    info!("POST /webhook/{} - Received webhook for source", service);

    let mut headers = HashMap::new();
    for (header_name, header_value) in req.headers() {
        if let Ok(value_str) = header_value.to_str() {
            headers.insert(header_name.to_string(), value_str.to_string());
        }
    }

    let webhook_event = WebhookEvent {
        payload: payload.into_inner(),
        headers,
    };

    let endpoints = data.endpoints.read().unwrap();
    let matching: Vec<WebhookEndpoint> = endpoints
        .iter()
        .filter(|e| e.is_active && e.source.as_deref() == Some(&service))
        .cloned()
        .collect();

    if matching.is_empty() {
        warn!("No active endpoints found for source: {}", service);
        return HttpResponse::Ok().json(serde_json::json!({
            "status": "no_active_endpoints",
            "message": format!("No active endpoints configured for source: {}", service)
        }));
    }

    info!(
        "Forwarding webhook for source '{}' to {} endpoint(s)",
        service,
        matching.len()
    );

    let webhook_event_clone = webhook_event.clone();

    rt::spawn(async move {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let futures: Vec<_> = matching
            .into_iter()
            .map(|endpoint| {
                let client = client.clone();
                let payload = webhook_event_clone.clone();

                async move {
                    if let Err(error) = forward_webhook(&client, &endpoint, &payload).await {
                        error!("Error forwarding to {}: {}", endpoint.name, error);
                        (endpoint.name, error)
                    } else {
                        (endpoint.name, "Success".to_string())
                    }
                }
            })
            .collect();

        let results = future::join_all(futures).await;

        for (endpoint_name, result) in results {
            if result != "Success" {
                error!("  {}: {}", endpoint_name, result);
            } else {
                info!("  {}: {}", endpoint_name, result);
            }
        }
    });

    HttpResponse::Ok().json(serde_json::json!({
        "status": "accepted",
        "message": "Webhook received and processing started"
    }))
}

// Webhook receiver endpoint that forwards to active endpoints
async fn receive_webhook(
    payload: web::Json<serde_json::Value>,
    req: HttpRequest,
    data: web::Data<AppState>,
) -> HttpResponse {
    info!("POST /webhook - Received webhook");
    debug!("Webhook payload: {:?}", payload);

    let mut headers = HashMap::new();
    for (header_name, header_value) in req.headers() {
        if let Ok(value_str) = header_value.to_str() {
            headers.insert(header_name.to_string(), value_str.to_string());
        }
    }

    let webhook_event = WebhookEvent {
        payload: payload.into_inner(),
        headers,
    };

    let endpoints = data.endpoints.read().unwrap();
    let active_endpoints: Vec<WebhookEndpoint> =
        endpoints.iter().filter(|e| e.is_active).cloned().collect();

    if active_endpoints.is_empty() {
        warn!("No active endpoints configured for webhook forwarding");
        return HttpResponse::Ok().json(serde_json::json!({
            "status": "no_active_endpoints",
            "message": "No active endpoints configured"
        }));
    }

    info!("Forwarding webhook to {} active endpoint(s)", active_endpoints.len());

    let webhook_event_clone = webhook_event.clone();

    rt::spawn(async move {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let futures: Vec<_> = active_endpoints
            .into_iter()
            .map(|endpoint| {
                let client = client.clone();
                let payload = webhook_event_clone.clone();

                async move {
                    if let Err(error) = forward_webhook(&client, &endpoint, &payload).await {
                        error!("Error forwarding to {}: {}", endpoint.name, error);
                        (endpoint.name, error)
                    } else {
                        (endpoint.name, "Success".to_string())
                    }
                }
            })
            .collect();

        let results = future::join_all(futures).await;

        for (endpoint_name, result) in results {
            if result != "Success" {
                error!("  {}: {}", endpoint_name, result);
            } else {
                info!("  {}: {}", endpoint_name, result);
            }
        }
    });

    HttpResponse::Ok().json(serde_json::json!({
        "status": "accepted",
        "message": "Webhook received and processing started"
    }))
}

// Infer a source from an endpoint URL/name (used to backfill older endpoints)
fn infer_source(url: &str, name: &str) -> Option<String> {
    let known_sources = [
        "fincra",
        "splice",
        "useorange",
        "galoy",
        "ibex",
        "nomba",
    ];

    if let Ok(parsed) = url::Url::parse(url) {
        if let Some(last_segment) = parsed
            .path_segments()
            .and_then(|mut segs| segs.next_back())
        {
            let normalized = last_segment.to_lowercase();
            if known_sources.contains(&normalized.as_str()) {
                return Some(normalized);
            }
        }
    }

    let haystack = format!("{} {}", url, name).to_lowercase();
    known_sources
        .iter()
        .find(|source| haystack.contains(*source))
        .map(|s| s.to_string())
}

// Save endpoints to a JSON file
fn save_endpoints(endpoints: &[WebhookEndpoint]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(endpoints)
        .map_err(|e| format!("Failed to serialize endpoints: {}", e))?;

    let path = endpoints_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Failed to create data dir: {}", e))?;
    }
    fs::write(&path, json).map_err(|e| format!("Failed to write endpoints file: {}", e))
}

// Load endpoints from a JSON file
fn load_endpoints() -> Vec<WebhookEndpoint> {
    let path = endpoints_path();
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<Vec<WebhookEndpoint>>(&contents) {
                Ok(mut endpoints) => {
                    // Backfill source for legacy endpoints that predate the source field.
                    let mut backfilled = false;
                    for endpoint in endpoints.iter_mut() {
                        if endpoint.source.is_none() {
                            if let Some(source) = infer_source(&endpoint.url, &endpoint.name) {
                                backfilled = true;
                                info!(
                                    "Backfilled source '{}' for endpoint: id={}, name={}",
                                    source, endpoint.id, endpoint.name
                                );
                                endpoint.source = Some(source);
                            }
                        }
                    }
                    if backfilled {
                        if let Err(e) = save_endpoints(&endpoints) {
                            error!("Error saving backfilled endpoints: {}", e);
                        }
                    }
                    info!("Loaded {} endpoints from file", endpoints.len());
                    return endpoints;
                }
                Err(e) => error!("Error parsing endpoints file: {}", e),
            },
            Err(e) => error!("Error reading endpoints file: {}", e),
        }
    }

    // Return default endpoints with our staging URLs
    let default_endpoints = vec![
        WebhookEndpoint {
            id: "fincra".to_string(),
            url: "https://staging.webhook.api.mavapay.co/webhook/fincra".to_string(),
            name: "Fincra Staging".to_string(),
            is_active: true,
            source: Some("fincra".to_string()),
        },
        WebhookEndpoint {
            id: "splice".to_string(),
            url: "https://staging.webhook.api.mavapay.co/webhook/splice".to_string(),
            name: "Splice Staging".to_string(),
            is_active: true,
            source: Some("splice".to_string()),
        },
        WebhookEndpoint {
            id: "useorange".to_string(),
            url: "https://staging.webhook.api.mavapay.co/webhook/useorange".to_string(),
            name: "UseOrange Staging".to_string(),
            is_active: true,
            source: Some("useorange".to_string()),
        },
        WebhookEndpoint {
            id: "galoy".to_string(),
            url: "https://staging.webhook.api.mavapay.co/webhook/galoy".to_string(),
            name: "Galoy Staging".to_string(),
            is_active: true,
            source: Some("galoy".to_string()),
        },
        WebhookEndpoint {
            id: "ibex".to_string(),
            url: "https://staging.webhook.api.mavapay.co/webhook/ibex".to_string(),
            name: "Ibex Staging".to_string(),
            is_active: true,
            source: Some("ibex".to_string()),
        },
        WebhookEndpoint {
            id: "nomba".to_string(),
            url: "https://staging.webhook.api.mavapay.co/webhook/nomba".to_string(),
            name: "Nomba Staging".to_string(),
            is_active: true,
            source: Some("nomba".to_string()),
        },
    ];

    // Save the default endpoints
    if let Err(e) = save_endpoints(&default_endpoints) {
        error!("Error saving default endpoints: {}", e);
    }

    default_endpoints
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let bind_address = format!("0.0.0.0:{}", port);

    info!("Starting webhook relay server on {}", bind_address);

    // Load endpoints from persistent storage
    let endpoints = load_endpoints();

    let app_state = web::Data::new(AppState {
        endpoints: RwLock::new(endpoints),
    });

    HttpServer::new(move || {
        let cors = Cors::permissive(); // For development only

        App::new()
            .wrap(cors)
            .wrap(middleware::from_fn(no_cache))
            .app_data(app_state.clone())
            .route("/webhook", web::post().to(receive_webhook))
            .route(
                "/webhook/{service}",
                web::post().to(handle_specific_webhook),
            )
            .route("/endpoints", web::post().to(register_endpoint))
            .route("/endpoints", web::get().to(list_endpoints))
            .route("/endpoints/{id}", web::delete().to(delete_endpoint))
            .route("/endpoints/{id}/status", web::put().to(update_endpoint))
            .service(actix_files::Files::new("/", "./static").index_file("index.html"))
    })
    .bind(&bind_address)?
    .run()
    .await
}
