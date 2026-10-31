# Phase 0 — Technical feasibility and risk reduction

## Objective

Resolve the architecture risks that could invalidate the proposed FreeBSD-native AI-first browser engine before committing to a full browser implementation.

## Workstreams

1. FreeBSD toolchain and reproducible builds.
2. JavaScript runtime candidates and embedding.
3. DOM ↔ JS lifetime model.
4. Broker / renderer security prototype.
5. Headless text, image and CPU raster pipeline.
6. Network / Fetch and outbound policy.
7. Native AI control protocol V1.
8. Initial Web capability matrix.
9. Dependency, licensing, CI and SBOM governance.

## Evidence rule

For each experiment, record the exact source revision, OS release, compiler/tool versions, command, exit status, stdout/stderr, test count, and known limitations. A build is not a compatibility result; a unit test is not a security proof; an architecture proposal is not an implemented feature.

## Exit gate

See the parent architecture blueprint's Phase 0 exit criteria. Do not proceed to full runtime implementation while critical DOM/JS lifetime or renderer isolation risks remain unexplained.
