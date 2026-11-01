use std::io::{self, Read};

use aibe_html_parser_poc::{MAX_HTML_INPUT_BYTES, parse_html};

fn main() {
    let mut input = Vec::new();
    if let Err(error) = io::stdin()
        .take((MAX_HTML_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut input)
    {
        eprintln!("failed to read HTML from stdin: {error}");
        std::process::exit(1);
    }

    let document = match parse_html(&input) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("HTML parse rejected: {error}");
            std::process::exit(1);
        }
    };
    let title = document
        .find_first_element_by_tag_name("title")
        .and_then(|node| document.text_content(node))
        .unwrap_or_default();
    let body_text = document
        .find_first_element_by_tag_name("body")
        .and_then(|node| document.text_content(node))
        .unwrap_or_default();
    println!("html_nodes={}", document.node_count());
    println!("quirks_mode={:?}", document.quirks_mode());
    println!("parse_errors={}", document.parse_errors().len());
    println!(
        "suppressed_parse_errors={}",
        document.suppressed_parse_errors()
    );
    println!("title={title:?}");
    println!("body_text={body_text:?}");
}
