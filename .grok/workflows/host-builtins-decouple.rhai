let meta = #{
    name: "host-builtins-decouple",
    description: "Execute docs/plan.md: decouple host_imports builtins into wjsm-builtins via ExecContext, phase by phase",
    when_to_use: "When running the Host Builtins full decoupling plan (docs/plan.md) for a phase or all phases",
    phases: [
        #{ title: "Scout", detail: "assess workspace vs plan progress" },
        #{ title: "Implement", detail: "execute one plan phase" },
        #{ title: "Verify", detail: "build + targeted/workspace tests" },
        #{ title: "Fix", detail: "repair verification failures" },
        #{ title: "Review", detail: "spec compliance for the phase" },
        #{ title: "Report", detail: "write phase report" },
    ],
};

// ── schemas ──

let scout_schema = #{
    "type": "object",
    "required": ["status", "completed_phases", "active_phase", "blockers", "notes"],
    "properties": #{
        "status": #{ "type": "string" },
        "completed_phases": #{ "type": "array", "items": #{ "type": "integer" } },
        "active_phase": #{ "type": "integer" },
        "blockers": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 8 },
        "notes": #{ "type": "string" },
    },
};

let implement_schema = #{
    "type": "object",
    "required": ["phase", "done", "files_changed", "summary", "open_issues"],
    "properties": #{
        "phase": #{ "type": "integer" },
        "done": #{ "type": "boolean" },
        "files_changed": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 40 },
        "summary": #{ "type": "string" },
        "open_issues": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 12 },
    },
};

let verify_schema = #{
    "type": "object",
    "required": ["passed", "commands", "failures", "evidence"],
    "properties": #{
        "passed": #{ "type": "boolean" },
        "commands": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 12 },
        "failures": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 20 },
        "evidence": #{ "type": "string" },
    },
};

let review_schema = #{
    "type": "object",
    "required": ["approved", "gaps", "notes"],
    "properties": #{
        "approved": #{ "type": "boolean" },
        "gaps": #{ "type": "array", "items": #{ "type": "string" }, "maxItems": 12 },
        "notes": #{ "type": "string" },
    },
};

// ── args ──
// args.phase: integer 0..6, or string "all" (default 0)
// args.max_fix_rounds: integer (default 2)
// args.skip_review: bool (default false)
// args.plan_path: string (default docs/plan.md)

let plan_path = "docs/plan.md";
let phase_arg = 0;
let max_fix_rounds = 2;
let skip_review = false;
let run_all = false;

if args != () {
    if args.plan_path != () { plan_path = args.plan_path; }
    if args.max_fix_rounds != () { max_fix_rounds = args.max_fix_rounds; }
    if args.skip_review != () { skip_review = args.skip_review; }
    if args.phase != () {
        if type_of(args.phase) == "string" {
            if args.phase == "all" {
                run_all = true;
                phase_arg = 0;
            } else {
                // numeric string
                phase_arg = args.phase.parse_int();
            }
        } else {
            phase_arg = args.phase;
        }
    }
}

if phase_arg < 0 || phase_arg > 6 {
    pause("verification", "args.phase must be 0..6 or \"all\"");
}

// ── phase descriptions (mirrors docs/plan.md) ──

fn phase_title(p) {
    if p == 0 { "Phase 0: infrastructure (wjsm-builtins + ExecContext + WasmExecContext + pure fns)" }
    else if p == 1 { "Phase 1: low-coupling builtins (math_number_error, primitive_core, get_method early, …)" }
    else if p == 2 { "Phase 2: callback builtins (string_methods, object_builtins, private_fields, …)" }
    else if p == 3 { "Phase 3: reentrant array/typedarray (reentrant/array, array_object, collections_buffers, …)" }
    else if p == 4 { "Phase 4: Proxy/Reflect (proxy_traps, proxy_reflect, proxy_reflect_async, …)" }
    else if p == 5 { "Phase 5: Promise/Streams/Fetch/Modules/Atomics/Core" }
    else if p == 6 { "Phase 6: cleanup, thin registration layer, AGENTS.md, ADR 0012" }
    else { "unknown phase" }
}

fn phase_verify_hint(p) {
    if p == 0 {
        "Run: cargo build --workspace 2>&1 | tail -50; cargo nextest run --workspace. Expect full green; no logic migration yet."
    } else if p == 1 {
        "Run: cargo build -p wjsm-host-wasm -p wjsm-builtins; cargo nextest run --workspace. Prefer filters happy__math / happy__number / happy__error if full suite is too slow as first pass, then full workspace."
    } else if p == 2 {
        "Run: cargo nextest run --workspace. Spot-check happy__string / happy__object / private fields fixtures."
    } else if p == 3 {
        "Run: cargo nextest run -E 'test(happy__array)' then cargo nextest run --workspace. Optional: cargo run -p wjsm-gc-bench -- run --scenario iteration --gc mark-sweep."
    } else if p == 4 {
        "Run: cargo nextest run -E 'test(happy__proxy)' ; cargo nextest run -E 'test(happy__reflect)' ; then cargo nextest run --workspace."
    } else if p == 5 {
        "Run: cargo nextest run --workspace. Cover promise/stream/fetch/modules/atomics/core paths."
    } else {
        "Run: cargo nextest run --workspace; WJSM_STARTUP_SNAPSHOT=1 cargo nextest run -E 'test(startup_snapshot)' if that filter exists; fix warnings to zero."
    }
}

fn phase_file_list(p) {
    if p == 0 {
        "Create crates/wjsm-builtins; wjsm-host/src/exec_context.rs; wjsm-host-wasm/src/exec_context_impl.rs; migrate pure property_key / RuntimeString / number format free functions per plan table."
    } else if p == 1 {
        "host_imports: math_number_error.rs, primitive_core.rs, get_builtin_global_entry.rs, inspector_host.rs, generator.rs, async_fn.rs, async_generator.rs, object_builtins_async.rs; ALSO advance get_method.rs early (risk 8)."
    } else if p == 2 {
        "host_imports: string_methods.rs, object_builtins.rs, timers_arrays.rs, weakref_finalization.rs, core_async.rs, misc.rs, private_fields.rs, reentrant/string.rs (or reentrant_async/string)."
    } else if p == 3 {
        "host_imports: reentrant/array.rs first, then reentrant/typedarray.rs, typedarray_new_methods.rs, array_object.rs, collections_buffers.rs."
    } else if p == 4 {
        "host_imports: proxy_traps.rs → proxy_reflect.rs → proxy_reflect_async.rs → reentrant/proxy.rs → reentrant/mod.rs."
    } else if p == 5 {
        "host_imports: promise*, streams/*, fetch/*, modules.rs, atomics.rs, gc.rs, get_method.rs (if not done), core.rs last."
    } else {
        "Thin host_imports registration layer only; merge WasmHeapContext into WasmExecContext if plan allows; update workspace Cargo.toml, AGENTS.md/CLAUDE.md crate table; write docs/adr/0012-host-builtins-decouple.md."
    }
}

fn phase_non_goals(p) {
    let base = "NON-GOALS for every phase: do not move shared helpers (alloc_host_object/define_host_data_property/store_runtime_string/settle_promise) into wjsm-builtins (risk 6 — leave in host-wasm, expose via ExecContext); do not move NativeCallable enum out of host-wasm types.rs (risk 7); no dyn ExecContext (must be generic monomorphization); no partial file migrations; no stub/TODO implementations; do not create git worktrees; comments in Chinese; no defensive 'minimal change' half-measures — complete the phase thoroughly; temporary artifacts only under /tmp.";
    if p == 0 {
        base + " Phase 0 non-goals: do not migrate any host_imports builtin logic yet (except pure free functions listed in plan)."
    } else if p == 3 {
        base + " Phase 3: after migration, performance must not regress vs monomorphized design (no vtable)."
    } else if p == 6 {
        base + " Phase 6 non-goals: do not re-migrate working code; only cleanup + docs + final verification."
    } else {
        base
    }
}

// ── Scout ──

phase("Scout");
log("Scouting workspace against " + plan_path + " (requested phase=" + phase_arg.to_string() + ", all=" + run_all.to_string() + ")");

let scout_prompt = "";
scout_prompt += "You are scouting progress for the Host Builtins decoupling plan.\n";
scout_prompt += "MUST read these files with tools (read_file/grep/list_dir), do not answer from memory:\n";
scout_prompt += "1) " + plan_path + " (migration order phases 0-6)\n";
scout_prompt += "2) crates/ — does wjsm-builtins exist?\n";
scout_prompt += "3) crates/wjsm-host/src/ — is exec_context.rs present?\n";
scout_prompt += "4) crates/wjsm-host-wasm/src/ — is exec_context_impl.rs present?\n";
scout_prompt += "5) crates/wjsm-host-wasm/src/host_imports/ — sample a few files: still full logic or thin registration?\n";
scout_prompt += "6) Cargo.toml workspace members for wjsm-builtins.\n";
scout_prompt += "Return JSON: status (not_started|in_progress|blocked|done), completed_phases (ints), active_phase (next to implement), blockers, notes.\n";
scout_prompt += "active_phase must be the next incomplete phase in 0..6. Empty completed_phases if nothing done.\n";

let scout = agent(scout_prompt, #{
    label: "scout",
    capability_mode: "read-only",
    output_schema: scout_schema,
});

let start_phase = phase_arg;
if scout != () && scout.success && scout.output != () {
    log("Scout: status=" + scout.output.status + " active_phase=" + scout.output.active_phase.to_string());
    if run_all {
        start_phase = scout.output.active_phase;
    }
} else {
    log("Scout failed or empty — defaulting to requested phase " + phase_arg.to_string());
}

if start_phase < 0 { start_phase = 0; }
if start_phase > 6 { start_phase = 6; }

let end_phase = start_phase;
if run_all { end_phase = 6; }

let phase_reports = [];
let p = start_phase;

while p <= end_phase {
    log("========== " + phase_title(p) + " ==========");

    // ── Implement ──
    phase("Implement");
    let impl_prompt = "";
    impl_prompt += "You are implementing ONE phase of docs/plan.md for the wjsm AOT JS/TS→WASM runtime.\n";
    impl_prompt += "Workspace root is the git repo. Work directly in the main working tree (no worktrees).\n\n";
    impl_prompt += "PLAN PATH: " + plan_path + "\n";
    impl_prompt += "ACTIVE PHASE: " + p.to_string() + " — " + phase_title(p) + "\n";
    impl_prompt += "FILES / SCOPE:\n" + phase_file_list(p) + "\n\n";
    impl_prompt += phase_non_goals(p) + "\n\n";
    impl_prompt += "ARCHITECTURE (mandatory):\n";
    impl_prompt += "- wjsm-ir ← wjsm-host (ExecContext + shared types) ← wjsm-builtins (<E: ExecContext> generic impls) ← wjsm-host-wasm (WasmExecContext + thin linker registration)\n";
    impl_prompt += "- Zero vtable: pub fn foo<E: ExecContext>(ctx: &mut E, ...)\n";
    impl_prompt += "- WasmExecContext holds &mut Caller<RuntimeState> + WasmEnv extracted once; methods delegate to existing host-wasm helpers\n";
    impl_prompt += "- Registration: define_* stays in host-wasm; each host func body becomes WasmExecContext::new + wjsm_builtins::...\n";
    impl_prompt += "- Shared helpers stay in host-wasm; builtins only call trait methods\n";
    impl_prompt += "- NativeCallable stays in host-wasm types.rs\n\n";
    impl_prompt += "TRUE MIGRATION RULES (fail if violated — prior runs failed review on these):\n";
    impl_prompt += "1. FORBIDDEN paper migration: do NOT leave algorithm bodies in host_imports and only wrap them as ctx.object_keys_async / ctx.op_in_async one-liners in wjsm-builtins.\n";
    impl_prompt += "2. FORBIDDEN trait bloat: do NOT add high-level ECMAScript algorithms onto ExecContext (Object.keys/assign/create, String.prototype methods, Array exotic full methods, op_in full Proxy walk as a single opaque trait method, etc.).\n";
    impl_prompt += "3. ALLOWED on ExecContext: low-level backend ops only — heap alloc/read/write, property slot get/set by name_id, store/read string, call_js/call_js_async, proxy_entry, settle_promise, native callable dispatch, GC safepoint, handle resolve, error/exception, iterator table step primitives.\n";
    impl_prompt += "4. REQUIRED: move the actual algorithm control flow into wjsm-builtins generic functions; host_imports files for this phase must be thin define_* + WasmExecContext::new + one-line call (no leftover 100+ LOC Caller-coupled impl helpers in those files).\n";
    impl_prompt += "5. If logic currently lives in host_imports/*_impl helpers, MOVE that source into wjsm-builtins (rewrite against ExecContext), then delete or shrink host helpers to zero for the migrated surface.\n";
    impl_prompt += "6. Callbacks: replace call_wasm_callback_async / block_in_place+block_on with ctx.call_js_async / ctx.call_js.\n";
    impl_prompt += "7. done=true only when EVERY file in this phase scope is truly migrated per rules 1-6. Partial stubs → done=false + open_issues.\n\n";
    impl_prompt += "PROCEDURE:\n";
    impl_prompt += "1. Read " + plan_path + " sections for this phase + risks 1-9.\n";
    impl_prompt += "2. Read existing patterns: crates/wjsm-host/src/heap_context.rs, crates/wjsm-host-wasm/src/host_imports/, exec_context_impl.rs.\n";
    impl_prompt += "3. For each file in scope: move algorithm to wjsm-builtins; thin host registration; remove host-side algorithm leftovers.\n";
    impl_prompt += "4. After edits: cargo build for affected crates; fix compiler errors and warnings you introduce.\n";
    impl_prompt += "5. Run targeted nextest for touched surfaces, then broader suite if time allows.\n";
    impl_prompt += "6. Self-check: rg for leftover thick bodies in host_imports files of this phase; rg for high-level object_*/string_* algorithms newly added to ExecContext — remove them.\n\n";
    impl_prompt += "PROJECT RULES: Rust 2024; comments Chinese; no stubs; match ECMAScript where semantics touched; zero-warning builds; use cargo nextest not cargo test.\n\n";
    impl_prompt += "Return JSON: phase, done (true only if phase scope is complete under TRUE MIGRATION RULES), files_changed, summary, open_issues.\n";

    let impl_r = agent(impl_prompt, #{
        label: "implement-p" + p.to_string(),
        capability_mode: "all",
        output_schema: implement_schema,
    });

    let impl_done = false;
    let impl_summary = "implement agent failed";
    if impl_r != () && impl_r.success && impl_r.output != () {
        impl_done = impl_r.output.done;
        impl_summary = impl_r.output.summary;
        log("Implement phase " + p.to_string() + " done=" + impl_done.to_string() + ": " + impl_summary);
    } else {
        log("Implement phase " + p.to_string() + " agent failed");
    }

    // ── Verify ──
    phase("Verify");
    let verify_prompt = "";
    verify_prompt += "You are the verification gate for Host Builtins decoupling phase " + p.to_string() + ".\n";
    verify_prompt += "Use shell tools to run real commands. Fail closed: if you cannot produce evidence, passed=false.\n";
    verify_prompt += phase_verify_hint(p) + "\n";
    verify_prompt += "Also: cargo build -p wjsm-host -p wjsm-host-wasm 2>&1 | tail -80 (and -p wjsm-builtins if the crate exists).\n";
    verify_prompt += "Fix nothing in this agent — only measure. Report exact failure lines in failures[].\n";
    verify_prompt += "evidence must quote command exit codes and key output snippets you actually observed.\n";
    verify_prompt += "Return JSON: passed, commands, failures, evidence.\n";

    let ver = agent(verify_prompt, #{
        label: "verify-p" + p.to_string(),
        capability_mode: "execute",
        output_schema: verify_schema,
    });

    let passed = false;
    let ver_evidence = "";
    if ver != () && ver.success && ver.output != () {
        passed = ver.output.passed;
        ver_evidence = ver.output.evidence;
        log("Verify phase " + p.to_string() + " passed=" + passed.to_string());
    } else {
        log("Verify phase " + p.to_string() + " agent failed — treating as not passed");
        passed = false;
    }

    // ── Fix loop ──
    let fix_round = 0;
    while passed == false && fix_round < max_fix_rounds {
        phase("Fix");
        fix_round += 1;
        log("Fix round " + fix_round.to_string() + " for phase " + p.to_string());

        let fix_prompt = "";
        fix_prompt += "Verification FAILED for Host Builtins decoupling phase " + p.to_string() + ".\n";
        fix_prompt += "Phase: " + phase_title(p) + "\n";
        fix_prompt += "Verification evidence:\n" + ver_evidence + "\n\n";
        if ver != () && ver.success && ver.output != () && ver.output.failures != () {
            fix_prompt += "Failure list from verifier (JSON-encoded):\n";
            fix_prompt += json_encode(ver.output.failures);
            fix_prompt += "\n\n";
        }
        fix_prompt += "Fix the root causes. Re-run the failing commands until they pass or you exhaust genuine blockers.\n";
        fix_prompt += "Respect architecture non-goals: helpers + NativeCallable stay in host-wasm; generics not dyn; no stubs.\n";
        fix_prompt += "Read " + plan_path + " risk mitigations if the failure is architectural.\n";
        fix_prompt += "Return JSON: phase, done (true if you believe tests should pass), files_changed, summary, open_issues.\n";

        let fix_r = agent(fix_prompt, #{
            label: "fix-p" + p.to_string() + "-r" + fix_round.to_string(),
            capability_mode: "all",
            output_schema: implement_schema,
        });

        if fix_r != () && fix_r.success && fix_r.output != () {
            log("Fix summary: " + fix_r.output.summary);
        }

        // re-verify
        phase("Verify");
        let reverify_prompt = "";
        reverify_prompt += "Re-verify phase " + p.to_string() + " after fix round " + fix_round.to_string() + ".\n";
        reverify_prompt += "Run real commands. Fail closed.\n";
        reverify_prompt += phase_verify_hint(p) + "\n";
        reverify_prompt += "Return JSON: passed, commands, failures, evidence.\n";

        ver = agent(reverify_prompt, #{
            label: "reverify-p" + p.to_string() + "-r" + fix_round.to_string(),
            capability_mode: "execute",
            output_schema: verify_schema,
        });

        passed = false;
        ver_evidence = "";
        if ver != () && ver.success && ver.output != () {
            passed = ver.output.passed;
            ver_evidence = ver.output.evidence;
            log("Re-verify passed=" + passed.to_string());
        }
    }

    if passed == false {
        let blocked_report = "";
        blocked_report += "# Phase " + p.to_string() + " BLOCKED\n\n";
        blocked_report += phase_title(p) + "\n\n";
        blocked_report += "## Implement summary\n" + impl_summary + "\n\n";
        blocked_report += "## Last verification evidence\n" + ver_evidence + "\n\n";
        blocked_report += "Fix rounds exhausted: " + max_fix_rounds.to_string() + "\n";
        let blocked_path = write_scratch_file("phase-" + p.to_string() + "-blocked.md", blocked_report);
        phase_reports.push(#{
            phase: p,
            status: "blocked",
            path: blocked_path,
        });
        // stop the run — later phases depend on earlier ones
        complete(#{
            status: "blocked",
            stopped_at_phase: p,
            phase_reports: phase_reports,
            message: "Phase " + p.to_string() + " failed verification after " + max_fix_rounds.to_string() + " fix rounds. Resume with args.phase=" + p.to_string() + " after inspection.",
        });
    }

    // ── Review (phase end) ──
    let review_approved = true;
    let review_notes = "skipped";
    if skip_review == false {
        phase("Review");
        let review_prompt = "";
        review_prompt += "Spec-compliance review for Host Builtins decoupling phase " + p.to_string() + ".\n";
        review_prompt += "Read " + plan_path + " phase " + p.to_string() + " checklist and compare to the working tree with read-only tools.\n";
        review_prompt += "Scope: " + phase_file_list(p) + "\n";
        review_prompt += "Approve only if ALL hold:\n";
        review_prompt += "(1) Every file in phase scope is truly migrated — algorithm control flow lives in wjsm-builtins generic code, not in host_imports leftover helpers.\n";
        review_prompt += "(2) host_imports files for this phase are thin define_*/WasmExecContext wrappers (no multi-hundred-line Caller-coupled impl remaining in those files).\n";
        review_prompt += "(3) ExecContext was NOT bloated with high-level ES algorithms for this phase (Object.keys/assign, full op_in opaque method, etc.). Only low-level backend ops are acceptable on the trait.\n";
        review_prompt += "(4) Paper migration REJECT: wjsm-builtins one-liners that only call ctx.high_level_async where the real body still sits in host-wasm.\n";
        review_prompt += "(5) Architecture: generics not dyn; helpers + NativeCallable stay in host-wasm; no todo!/unimplemented! on migrated surface.\n";
        review_prompt += "(6) Callbacks use ctx.call_js / call_js_async rather than direct call_wasm_callback_async in builtins.\n";
        review_prompt += "Be adversarial. gaps[] must be concrete file:issue. approved=true only with evidence you inspected.\n";
        review_prompt += "Return JSON: approved, gaps, notes.\n";

        let rev = agent(review_prompt, #{
            label: "review-p" + p.to_string(),
            capability_mode: "read-only",
            output_schema: review_schema,
        });

        review_approved = false;
        if rev != () && rev.success && rev.output != () {
            review_approved = rev.output.approved;
            review_notes = rev.output.notes;
            if review_approved == false && rev.output.gaps != () {
                log("Review gaps: " + json_encode(rev.output.gaps));
            }
        }

        if review_approved == false {
            phase("Fix");
            let revfix_prompt = "";
            revfix_prompt += "Spec review REJECTED phase " + p.to_string() + ". Close the gaps completely.\n";
            revfix_prompt += "Review notes: " + review_notes + "\n";
            if rev != () && rev.success && rev.output != () && rev.output.gaps != () {
                revfix_prompt += "Gaps JSON: " + json_encode(rev.output.gaps) + "\n";
            }
            revfix_prompt += "Then re-run: cargo build for affected crates and a reasonable nextest slice.\n";
            revfix_prompt += "Return JSON implement_schema fields.\n";

            let _rf = agent(revfix_prompt, #{
                label: "review-fix-p" + p.to_string(),
                capability_mode: "all",
                output_schema: implement_schema,
            });

            // one re-review
            let rev2 = agent(review_prompt + "\nThis is a re-review after gap fixes.\n", #{
                label: "rereview-p" + p.to_string(),
                capability_mode: "read-only",
                output_schema: review_schema,
            });
            review_approved = false;
            if rev2 != () && rev2.success && rev2.output != () {
                review_approved = rev2.output.approved;
                review_notes = rev2.output.notes;
            }
            if review_approved == false {
                let gap_report = "# Phase " + p.to_string() + " REVIEW NOT APPROVED\n\n" + review_notes + "\n";
                let gap_path = write_scratch_file("phase-" + p.to_string() + "-review-gaps.md", gap_report);
                phase_reports.push(#{
                    phase: p,
                    status: "review_failed",
                    path: gap_path,
                });
                complete(#{
                    status: "review_failed",
                    stopped_at_phase: p,
                    phase_reports: phase_reports,
                    message: "Phase " + p.to_string() + " passed tests but failed spec review. Resume with args.phase=" + p.to_string() + ".",
                });
            }
        }
    }

    // ── per-phase report ──
    phase("Report");
    let report = "";
    report += "# Phase " + p.to_string() + " complete\n\n";
    report += phase_title(p) + "\n\n";
    report += "## Implement\n" + impl_summary + "\n\n";
    report += "## Verification\npassed=true\n\n" + ver_evidence + "\n\n";
    report += "## Review\napproved=" + review_approved.to_string() + "\n" + review_notes + "\n";
    let rpath = write_scratch_file("phase-" + p.to_string() + "-report.md", report);
    phase_reports.push(#{
        phase: p,
        status: "complete",
        path: rpath,
    });
    log("Phase " + p.to_string() + " complete → " + rpath);

    p += 1;
}

// ── final ──
let final_text = "# host-builtins-decouple run complete\n\n";
final_text += "Phases " + start_phase.to_string() + ".." + end_phase.to_string() + " finished green.\n\n";
final_text += "Reports:\n";
let i = 0;
while i < phase_reports.len() {
    let pr = phase_reports[i];
    final_text += "- phase " + pr.phase.to_string() + " [" + pr.status + "]: " + pr.path + "\n";
    i += 1;
}
final_text += "\nNext: if not through phase 6, re-run with args.phase set to next integer, or args.phase=\"all\".\n";
let final_path = write_scratch_file("final-report.md", final_text);

complete(#{
    status: "ok",
    start_phase: start_phase,
    end_phase: end_phase,
    phase_reports: phase_reports,
    path: final_path,
    report: final_text,
});
