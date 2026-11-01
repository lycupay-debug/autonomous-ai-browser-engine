use std::path::Path;

use aibe_network_policy_poc::fetch_https_headers;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(url) = args.next() else {
        eprintln!("usage: aibe-pinned-https-probe <https-url> [ca-bundle]");
        std::process::exit(2);
    };
    let ca_bundle = args.next().unwrap_or_else(|| "/etc/ssl/cert.pem".into());
    match fetch_https_headers(&url, Path::new(&ca_bundle), 5) {
        Ok(result) => {
            println!("status={}", result.status);
            println!("final_url={}", result.final_url);
            println!("connected_peer={}", result.connected_peer);
            println!("redirects={}", result.redirect_chain.len());
            for (status, url) in result.redirect_chain {
                println!("redirect={status} {url}");
            }
            for (name, value) in result.headers {
                println!("header={name}: {value}");
            }
        }
        Err(error) => {
            eprintln!("request denied/failed: {error}");
            std::process::exit(1);
        }
    }
}
