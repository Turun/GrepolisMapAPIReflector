use axum::{
    body::Body,
    extract::Path,
    http::{Response, StatusCode},
    response::IntoResponse,
    routing::get,
    Extension, Router,
};
use bytes::Bytes;
use lazy_static::lazy_static;
use regex::Regex;
use reqwest::Client;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{fs, sync::RwLock};

const CACHE_EXPIRY: Duration = Duration::from_secs(15 * 60); // 15 minutes
const MAX_FILES_IN_RAM_CACHE: usize = 25;
lazy_static! {
    static ref SERVER_REGEX: Regex = Regex::new(r"^[a-zA-Z]{2}\d{1,3}$").unwrap();
    static ref DATAFILE_WHITELIST: Vec<&'static str> =
        vec!["players.txt", "towns.txt", "alliances.txt", "islands.txt"];
}

#[tokio::main]
async fn main() {
    // Initialize the cache and HTTP client
    let app_state = Arc::new(AppState::new().await);
    // Build our application with a route
    let app = Router::new()
        .route("/{server}/{datafile}", get(handle_request))
        .layer(Extension(app_state));

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}

struct AppState {
    cache: RwLock<HashMap<String, CacheEntry>>,
    failed_cache: RwLock<HashMap<String, Instant>>,
    client: Client,
    cache_dir: PathBuf,
}

impl AppState {
    async fn new() -> Self {
        // Create the HTTP client with custom headers
        let client = Client::builder()
            .user_agent("YourCustomUserAgent")
            .gzip(true)
            .deflate(true)
            .build()
            .unwrap();
        // Set up the cache directory
        let cache_dir = "./cache".into();
        fs::create_dir_all(&cache_dir).await.unwrap();
        Self {
            cache: RwLock::new(HashMap::new()),
            failed_cache: RwLock::new(HashMap::new()),
            client,
            cache_dir,
        }
    }
}

struct CacheEntry {
    data: Bytes,
    timestamp: Instant,
}

async fn handle_request(
    Path((server, datafile)): Path<(String, String)>,
    Extension(state): Extension<Arc<AppState>>,
) -> Response<Body> {
    // Validate the server parameter
    if !SERVER_REGEX.is_match(&server) {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Validate the datafile parameter
    if !DATAFILE_WHITELIST.contains(&datafile.as_str()) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let cache_key = format!("{}/{}", server, datafile);

    // Check if there is a cached failure
    if let Some(failed_response) = get_from_failed_cache(&state, &cache_key).await {
        if failed_response.elapsed() < CACHE_EXPIRY {
            return StatusCode::BAD_GATEWAY.into_response();
        }
    }

    // Check if response is cached in RAM
    if let Some(data) = get_from_ram_cache(&state, &cache_key).await {
        return (StatusCode::OK, data).into_response();
    }
    // Check if response is cached on disk
    if let Some(data) = get_from_disk_cache(&state, &cache_key).await {
        // Update RAM cache
        update_ram_cache(&state, &cache_key, &data).await;
        return (StatusCode::OK, data).into_response();
    }
    // Fetch from the external API
    match fetch_and_cache(&state, &server, &datafile, &cache_key).await {
        Ok((status, data)) => (status, data).into_response(),
        Err(status) => status.into_response(), // if we're here it's always BAD_GATEWAY
    }
}

async fn get_from_ram_cache(state: &Arc<AppState>, cache_key: &str) -> Option<Bytes> {
    let cache = state.cache.read().await;
    if let Some(entry) = cache.get(cache_key) {
        if entry.timestamp.elapsed() < CACHE_EXPIRY {
            // Cache hit
            return Some(entry.data.clone());
        }
    }
    None
}

async fn get_from_disk_cache(state: &Arc<AppState>, cache_key: &str) -> Option<Bytes> {
    let cache_path = state.cache_dir.join(cache_key);
    if let Ok(metadata) = fs::metadata(&cache_path).await {
        if metadata.is_file() {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed < CACHE_EXPIRY {
                        if let Ok(data) = fs::read(&cache_path).await {
                            return Some(Bytes::from(data));
                        }
                    }
                }
            }
        }
    }
    None
}

async fn get_from_failed_cache(state: &Arc<AppState>, cache_key: &str) -> Option<Instant> {
    let cache = state.failed_cache.read().await;
    cache.get(cache_key).cloned()
}

async fn update_failed_cache(state: &Arc<AppState>, cache_key: &str) {
    let mut cache = state.failed_cache.write().await;
    cache.insert(cache_key.to_string(), Instant::now());
}

async fn fetch_and_cache(
    state: &Arc<AppState>,
    server: &str,
    datafile: &str,
    cache_key: &str,
) -> Result<(StatusCode, Bytes), StatusCode> {
    let url = format!("https://{}.grepolis.com/data/{}", server, datafile);

    // Perform the HTTP GET request with custom headers
    let response = match state.client.get(&url).send().await {
        Ok(resp) => resp,
        Err(_) => {
            update_failed_cache(state, cache_key).await;
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

    if !response.status().is_success() {
        update_failed_cache(state, cache_key).await;
        return Err(StatusCode::BAD_GATEWAY);
    }

    let data = match response.bytes().await {
        Ok(b) => b,
        Err(_) => {
            update_failed_cache(state, cache_key).await;
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

    // Update caches
    update_disk_cache(state, cache_key, &data).await;
    update_ram_cache(state, cache_key, &data).await;
    Ok((StatusCode::OK, data))
}

async fn update_ram_cache(state: &Arc<AppState>, cache_key: &str, data: &Bytes) {
    let mut cache = state.cache.write().await;
    // If the cache exceeds MAX_FILES_IN_RAM_CACHE, remove the least recently used entry
    if cache.len() >= MAX_FILES_IN_RAM_CACHE {
        // Simple LRU implementation
        if let Some(oldest_key) = cache
            .iter()
            .min_by_key(|entry| entry.1.timestamp)
            .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest_key);
        }
    }
    // Insert the new entry
    cache.insert(
        cache_key.to_string(),
        CacheEntry {
            data: data.clone(),
            timestamp: Instant::now(),
        },
    );
}

async fn update_disk_cache(state: &Arc<AppState>, cache_key: &str, data: &Bytes) {
    let cache_path = state.cache_dir.join(cache_key);
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent).await.ok();
    }
    fs::write(cache_path, data).await.ok();
}
