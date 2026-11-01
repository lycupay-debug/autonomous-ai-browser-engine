use std::path::Path;

use aibe_network_policy_poc::{FetchHeadersResult, fetch_https_get, fetch_https_headers};

fn print_metadata(result: &FetchHeadersResult) {
    println!("status={}", result.status);
    println!("final_url={}", result.final_url);
    println!("connected_peer={}", result.connected_peer);
    println!("redirects={}", result.redirect_chain.len());
    for (status, url) in &result.redirect_chain {
        println!("redirect={status} {url}");
    }
    for (name, value) in &result.headers {
        println!("header={name}: {value}");
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(url) = args.next() else {
        eprintln!("usage: aibe-network-policy-poc <https-url> [ca-bundle] [--body]");
        std::process::exit(2);
    };
    let remaining = args.collect::<Vec<_>>();
    let body_mode = remaining.iter().any(|argument| argument == "--body");
    let ca_bundle = remaining
        .iter()
        .find(|argument| argument.as_str() != "--body")
        .map(String::as_str)
        .unwrap_or("/etc/ssl/cert.pem");

    if body_mode {
        match fetch_https_get(&url, Path::new(ca_bundle), 5, 8 * 1024 * 1024) {
            Ok(result) => {
                print_metadata(&result.metadata);
                println!("body_bytes={}", result.body.len());
                let preview = result
                    .body
                    .iter()
                    .take(32)
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<Vec<_>>()
                    .join("");
                println!("body_prefix_hex={preview}");
            }
            Err(error) => {
                eprintln!("request denied/failed: {error}");
                std::process::exit(1);
            }
        }
    } else {
        match fetch_https_headers(&url, Path::new(ca_bundle), 5) {
            Ok(result) => print_metadata(&result),
            Err(error) => {
                eprintln!("request denied/failed: {error}");
                std::process::exit(1);
            }
        }
    }
}
