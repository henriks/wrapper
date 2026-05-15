# Epic wra-bjlw: Normalize generic share shadow configuration and recipe share templates

Implement the generic configured-share shadow model and move tool setup recipes toward explicit share templates rather than runtime special cases. Keep the implementation generic: shadows are configured mount composition, not Codex-specific runtime policy.

## Goals
- Change config `shares[].shadows` from object entries to string arrays.
- Allow shadows on both read-only and read-write parent shares.
- Make each shadow a read-write project-local substitution backed by `.sandbox/root/<full guest shadow path>`.
- Keep mount ordering deterministic so parent shares are mounted before more-specific shadows.
- Move setup recipes to emit explicit share template config for tool state needs.
- Reassess/close stale guest-home persistence ticket in light of the completed root overlay work.
- Update docs and tests to match the final model.
- Run `./vm-frontend/validate.sh required` before closing the epic.

## Tickets / Checklist
- [x] `wra-71xw` Switch share shadow config to string arrays.
- [x] `wra-cr8p` Make share shadows generic rw substitutions under `.sandbox/root`.
- [x] `wra-ieju` Update setup recipes to write explicit share templates.
- [x] `wra-q5lm` Reassess guest home and root persistence model.
- [x] `wra-mur9` Update shadow configuration documentation and validation coverage.
- [x] Run `./vm-frontend/validate.sh required` and record result.
- [x] Close completed tickets with notes.
- [x] Close epic `wra-bjlw` after validation passes.

## Reference Context
- Epic `wra-bjlw`: generic model, no Codex-specific runtime path handling.
- Current code context from ticket: `vm-frontend/src/main.rs` has `ConfigShare { host_path, guest_path, access, required, shadows: Vec<ConfigShareShadow> }`; `ConfigShareShadow` is currently object-form `{ path }`; hidden `--share-shadow` currently carries `PARENT_GUEST=RELATIVE=BACKING`.
- Current runtime backing from ticket: `.sandbox/share-shadows/share-NNNN/<relative>`; target backing: `<project>/.sandbox/root/<guest path without leading slash>`.
- Current docs to audit: `vm-frontend/config-json.md`, `requirements.md`, `docker/runtime-contract.md`, `wrapper-ux-contract.md`, `docker/OPERATIONS.md`.
- Root overlay epic `wra-ia1a` is complete: guest `$HOME`, `/usr/local`, caches, and `/var/lib/docker` persist in `.sandbox/docker-vm/state.raw`; `.sandbox/home` special persistent-home mount has been removed. Do not reintroduce special guest-home persistence while implementing share shadows.
- Project guidance: config compatibility is not required for this pre-user app; remove obsolete config formats instead of supporting both.
- Validation gate before completion: `./vm-frontend/validate.sh required`.

## Verification
- Iteration 1 targeted validation passed: `cargo fmt --manifest-path vm-frontend/Cargo.toml -- --check`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline config_share_shadow -- --nocapture`.
- Iteration 2 targeted validation passed: `./vm-frontend/validate.sh docs`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline setup_tool -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline config_share_shadow -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline legacy_config -- --nocapture`.
- Iteration 3 targeted validation passed: `cargo fmt --manifest-path vm-frontend/Cargo.toml`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline setup_tool -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline config_editor_model_edits_main_config_fields_before_save -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_parses_payload_guest_share_options -- --nocapture`; `cargo test --manifest-path vm-frontend/Cargo.toml --offline guest_runtime_mounts -- --nocapture`; `./vm-frontend/validate.sh docs`.
- Iteration 4 final validation passed: `./vm-frontend/validate.sh required` completed docs drift, rustfmt, composed-fs offline tests, vm-frontend offline tests, offline guest service tests, fuzz target compilation, and live-smoke.

## Notes
- Suggested implementation order follows ticket dependencies: `wra-71xw` -> `wra-cr8p` -> `wra-ieju` -> `wra-q5lm` -> `wra-mur9` -> required validation -> close epic.
- If ticket dependency metadata conflicts with the now-completed root overlay work, add notes to affected tickets before changing/closing them.
- Iteration 1: closed `wra-71xw`. `ConfigShare.shadows` is now `Vec<String>`, object-form shadows are obsolete and rejected by serde, duplicate normalized paths are rejected, and `vm-frontend/config-json.md` documents `"shadows": ["tmp"]`.
- Iteration 1: closed `wra-cr8p`. Shadows now work under ro or rw parent shares, generate always-rw nested `UserRw` mounts after the parent mount, and use project-local backing under `.sandbox/root/<full guest shadow path>`.
- Iteration 2: started `wra-ieju`. Setup recipes now write explicit optional rw shares: Codex maps host/guest `~/.codex` with `shadows: ["tmp"]`; Pi maps host/guest `~/.pi`. `tool_state` remains present but setup recipes now write it as false/default; remaining `wra-ieju` work is to remove or collapse old manual tool_state/runtime Codex/Pi path handling so tool-specific paths live only in recipe config generation.
- Iteration 2: closed `wra-q5lm` as resolved by completed root-overlay work: guest `$HOME` is ordinary root overlay state, `.sandbox/home` is gone, and share shadow backing remains a separate explicit `.sandbox/root/<guest path>` mechanism.
- Iteration 3: closed `wra-ieju`. Removed `tool_state` from config schema and `--tool-state` from frontend parsing; removed Codex/Pi `ToolStateMounts` and runtime-manifest path lists. Setup/TUI recipes now express tool-specific state through explicit generic shares only.
- Iteration 3: started `wra-mur9`; active docs no longer mention `tool_state`, `--tool-state`, object-form shadows, `.sandbox/share-shadows`, or rw-parent-only shadow rules. Remaining work is final validation and closing docs ticket after the required gate.
- Iteration 4: fixed remaining TUI tests after setup recipes gained explicit shares, ran the required gate successfully, closed `wra-mur9`, and closed epic `wra-bjlw`.
