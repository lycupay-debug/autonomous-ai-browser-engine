# Initial Web Capability Matrix — Phase 0 draft

This matrix defines what the first useful browser-engine vertical slice must prove. It does not claim standards conformance or commit to implementing every Web API.

| User task | Minimum engine capabilities | Phase 0 evidence and current status | Required evidence to exit | Initial priority |
|---|---|---|---|---|
| Read a static public page | URL parsing, HTTP(S), HTML parser, DOM tree, text extraction, basic CSS/layout | URL/Origin and bounded HTTPS response-body PoCs exist; no HTML parser, browser resource loader, or parser-backed DOM tree. **Not implemented end-to-end.** | Local HTML fixture parsed into DOM; text/links extracted; then a representative public page after security gates | P0 |
| Navigate links and redirects | URL resolution, navigation state, redirect policy, origin calculation, TLS verification | Origin Registry, Navigation lifecycle model, and bounded HTTPS redirect/header/body PoCs exist; Network Policy is not connected to Navigation. **Partial PoCs only.** | Loader-to-Navigation integration; every redirect revalidates DNS/IP and origin policy; body/error/cancel tests | P0 |
| Inspect a page for AI | Stable node IDs, DOM queries, semantic/accessibility representation, source/evidence mapping | V8 DOM wrapper/cache PoCs exist; Navigation now initializes a minimal `document` placeholder and native Node root only in Document Realms (not Worker Realms). No parser-backed DOM, stable browser Node IDs, snapshot/query API, or evidence service. **Partial isolated PoCs only.** | Parsed DOM fixture; deterministic queries; stale-handle rejection; structured evidence snapshots | P0 |
| Fill and submit forms | DOM controls, focus, keyboard/input semantics, form submission, validation events | No HTML parser, form controls, focus model, or browser input dispatch. **Not implemented.** | Synthetic fixture tests for input/change/submit, constraint validation, focus and navigation outcomes | P0 |
| Use a JavaScript-driven SPA | ECMAScript, DOM bindings, HTML task/microtask ordering, Fetch, timers, history | V8 FFI/Realm and DOM-wrapper PoCs plus explicit microtask checkpoint tests exist; Navigation installs a minimal placeholder Document in Document Realms. No HTML Event Loop, timers, browser Fetch service, History, or SPA integration. **Partial runtime PoCs only.** | Test262 subset plus parsed SPA fixture; task/microtask order; DOM mutation; Fetch and navigation integration | P0 |
| Work with authenticated business systems | Cookies, origin/storage partitioning, redirects, session lifecycle, policy-scoped private-network access | Origin grouping and per-Origin storage descriptor/security PoCs exist; no Cookie/Storage service, profile/session persistence, or approved private-network policy profile. **Partial security primitives only.** | Isolated business-system fixture; cookie/storage isolation; session lifecycle; private-network allow/deny audit | P1 |
| Download/upload files | Download broker, explicit path grants, upload file picker abstraction, quota and cleanup | No file broker or upload/download API. **Not implemented.** | File broker allow/deny tests; size/path quotas; no Renderer filesystem access | P1 |
| Interpret visual-only controls | Layout, paint, screenshots, accessibility/visual correlation | Swash text shaping/glyph rasterization and PNG/JPEG/GIF/WebP decoding exist; no CSS style/layout tree, display list, compositor, or page screenshot. **Partial raster PoC only.** | Deterministic geometry and screenshots; pixel/reftest; AI node-to-pixel correlation | P1 |
| Media-heavy pages | Image decode, audio/video, canvas, WebGL/WebGPU | Common image decoding exists; audio/video/canvas/WebGL/WebGPU do not. **Partial image PoC only.** | Capability-by-capability tests, decoding limits and resource budgets | P2 |
| Advanced platform APIs | Workers, service workers, WebRTC, WebSocket, notifications, device APIs | Dedicated Worker ownership/Realm lifecycle and loader-fed redirect-origin validation contracts exist; no script fetch/execution, WorkerGlobalScope, event loop, messaging, SharedWorker or ServiceWorker. **Lifecycle/security model only.** | Real loader integration, script execution, WorkerGlobalScope/event loop/message tests; separate review for each API | P2 |

## Initial product scope

- Two target environments must be supported by the architecture: ordinary public Internet websites and user-authorized private business systems.
- Private/loopback/link-local destinations are denied by default. Internal business access requires an explicit, narrow policy profile (approved hostnames/IP ranges, ports, duration, and task scope); no global private-network allowlist.
- The first vertical slice should use deterministic local fixtures for HTML, forms, SPA behavior, navigation, storage, and hostile page content. Public websites are added only after the network and renderer security gates pass.
- The initial engine is headless-first. Headed/GPU rendering is a later backend, not a prerequisite for validating the AI control protocol.
- AI control must be built into the runtime from the first useful slice: query, inspect, navigate, interact, wait, cancel, and return structured evidence.
- This matrix is a Phase 0 draft; the exact set of standards and APIs is not yet approved as a final compatibility target.


## Evidence status rules

- **Not implemented** means no executable project PoC currently proves the capability.
- **Partial PoCs only** means one or more isolated primitives are tested, but the complete user task is not wired end-to-end.
- A row is not considered implemented merely because an underlying dependency or API exists.
- Current Phase 0 evidence is recorded in `docs/phase-0/README.md`; the matrix is a scope/acceptance document, not a compatibility claim.
