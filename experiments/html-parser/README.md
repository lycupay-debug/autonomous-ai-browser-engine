# HTML Parser PoC

This crate uses `html5ever` 0.40.1 for HTML tokenization and WHATWG-style tree construction, with a project-owned index-based node arena implementing `TreeSink`. It does not use `markup5ever_rcdom`; that crate explicitly describes itself as an unsupported test-only DOM.

## Implemented

- Document, DocumentFragment, Doctype, Element, Text, Comment and ProcessingInstruction node records.
- HTML namespace tree construction, SVG/MathML foreign content, template contents, duplicate-attribute handling and quirks mode.
- Basic read-only queries by HTML tag, local name, `id`, attribute and text content.
- Bounded input (1 MiB), node count (65,536 including the Document root) and recorded parse errors (256). A sentinel prevents allocations after the node budget is exhausted; parsing returns a typed error instead of exposing a partial tree.
- `parse_html` assumes UTF-8 input. html5ever's `from_utf8()` decoder is lossy; encoding sniffing and non-UTF-8 document decoding are not implemented.

## Validation

`cargo test -p aibe-html-parser-poc --locked` covers implied document structure, malformed paragraph repair, named entities, raw-text elements, SVG/MathML namespaces, template contents, duplicate attributes and all three resource limits.

`./scripts/run-html-pipeline-poc.sh` performs a live bounded HTTPS GET in the Network Policy process, writes the response body to a temporary file, then runs the parser CLI in a separate process. The current fixture check fetched `https://example.com/` (HTTP 200, 577 bytes) and parsed 16 DOM nodes with title `Example Domain` and zero parse errors. The temporary file is removed on exit. This is a process-separated PoC handoff, not the final UDS Network Service protocol.

## Explicitly not implemented

- HTML encoding sniffing, streaming parse/`document.write`, script execution and parser-blocking script lifecycle.
- DOM mutation APIs, events, Web IDL bindings, JavaScript wrapper identity, layout, CSS, forms, focus or accessibility.
- Network Service IPC or automatic Navigation integration. `NavigationController` can attach a parsed tree to a live Document via `parse_document_html()` or atomically parse-before-commit via `commit_navigation_with_html()`, but the live network CLI handoff is currently a separate process/temporary-file experiment.
- The parser dependency is not a claim of full WHATWG HTML conformance. WPT/html5lib test suites and fuzzing remain required before untrusted public HTML can be treated as production-safe.

## Versioned DOM snapshot export

`HtmlDocument::to_snapshot_json()` exports a bounded, versioned flat snapshot (`schemaVersion: 1`) containing document-local node IDs, parent/child edges, node types, element namespaces, attributes, template-content references, parse diagnostics and quirks mode. The JSON payload is capped at 8 MiB. The flat representation avoids recursive serialization of adversarially deep trees and is suitable for a future authenticated IPC handoff.

Node IDs are valid only within the parsed Document generation. A caller must pair them with its own Document ID/revision and invalidate them when the tree is replaced. This is an immutable snapshot, not a live DOM, not a Web IDL binding, and not a cross-process authorization token. The serializer does not execute page scripts or create V8 wrappers.
