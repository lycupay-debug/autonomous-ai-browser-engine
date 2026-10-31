# Initial Web Capability Matrix — Phase 0 draft

This matrix defines what the first useful browser-engine vertical slice must prove. It does not claim standards conformance or commit to implementing every Web API.

| User task | Minimum engine capabilities | Required evidence | Initial priority |
|---|---|---|---|
| Read a static public page | URL parsing, HTTP(S), HTML parser, DOM tree, text extraction, basic CSS/layout | Local fixture + representative public page; parser and DOM tests | P0 |
| Navigate links and redirects | URL resolution, navigation state, redirect policy, origin calculation, TLS verification | Redirect chain tests; policy re-check at every hop | P0 |
| Inspect a page for AI | Stable node IDs, DOM queries, semantic/accessibility representation, source/evidence mapping | Deterministic query tests and evidence snapshots | P0 |
| Fill and submit forms | DOM controls, focus, keyboard/input semantics, form submission, validation events | Synthetic fixture tests for input/change/submit and validation | P0 |
| Use a JavaScript-driven SPA | ECMAScript, DOM bindings, HTML task/microtask ordering, Fetch, timers, history | Test262 subset + DOM/JS binding tests + SPA fixture | P0 |
| Work with authenticated business systems | Cookies, origin/storage partitioning, redirects, session lifecycle, policy-scoped private-network access | Isolated local business-system fixture; origin and session negative tests | P1 |
| Download/upload files | Download broker, explicit path grants, upload file picker abstraction, quota and cleanup | File broker allow/deny tests; no renderer filesystem access | P1 |
| Interpret visual-only controls | Layout, paint, screenshots, accessibility/visual correlation | Pixel/reftest and AI node-to-pixel correlation | P1 |
| Media-heavy pages | Image decode, audio/video, canvas, WebGL/WebGPU | Capability-by-capability tests and resource budgets | P2 |
| Advanced platform APIs | Workers, service workers, WebRTC, WebSocket, notifications, device APIs | Separate API support and security review | P2 |

## Initial product scope

- Two target environments must be supported by the architecture: ordinary public Internet websites and user-authorized private business systems.
- Private/loopback/link-local destinations are denied by default. Internal business access requires an explicit, narrow policy profile (approved hostnames/IP ranges, ports, duration, and task scope); no global private-network allowlist.
- The first vertical slice should use deterministic local fixtures for HTML, forms, SPA behavior, navigation, storage, and hostile page content. Public websites are added only after the network and renderer security gates pass.
- The initial engine is headless-first. Headed/GPU rendering is a later backend, not a prerequisite for validating the AI control protocol.
- AI control must be built into the runtime from the first useful slice: query, inspect, navigate, interact, wait, cancel, and return structured evidence.
- This matrix is a Phase 0 draft; the exact set of standards and APIs is not yet approved as a final compatibility target.
