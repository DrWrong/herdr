use super::*;

fn remote_manifest(version: &str, state: &str, contains: &str) -> String {
    format!(
        r#"
id = "codex"
version = "{version}"
min_engine_version = 1
updated_at = "2026-06-10T12:00:00Z"

[[rules]]
id = "test"
state = "{state}"
contains = ["{contains}"]
"#
    )
}

fn local_manifest(state: &str, contains: &str) -> String {
    format!(
        r#"
id = "codex"

[[rules]]
id = "test"
state = "{state}"
contains = ["{contains}"]
"#
    )
}

fn rules_manifest(rules: &str) -> String {
    format!(
        r#"
id = "codex"

{rules}
"#
    )
}

fn with_manifest_dirs<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let old_config = std::env::var_os("XDG_CONFIG_HOME");
    let old_state = std::env::var_os("XDG_STATE_HOME");
    let base = std::env::temp_dir().join(format!(
        "herdr-manifest-loader-{name}-{}",
        std::process::id()
    ));
    let config_dir = base.join("config");
    let state_dir = base.join("state");
    let _ = std::fs::remove_dir_all(&base);
    std::env::set_var("XDG_CONFIG_HOME", &config_dir);
    std::env::set_var("XDG_STATE_HOME", &state_dir);
    reload_manifests();
    let result = f();
    match old_config {
        Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
        None => std::env::remove_var("XDG_CONFIG_HOME"),
    }
    match old_state {
        Some(value) => std::env::set_var("XDG_STATE_HOME", value),
        None => std::env::remove_var("XDG_STATE_HOME"),
    }
    reload_manifests();
    let _ = std::fs::remove_dir_all(&base);
    result
}

fn write_remote_codex(content: &str) {
    let path = crate::detect::manifest_update::remote_manifest_path(Agent::Codex);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
    reload_manifests();
}

fn write_remote_codex_without_reload(content: &str) {
    let path = crate::detect::manifest_update::remote_manifest_path(Agent::Codex);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn write_local_codex(content: &str) {
    let path = override_path(Agent::Codex).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
    reload_manifests();
}

#[test]
fn known_agent_no_match_defaults_to_idle_fallback() {
    let explain = explain(Agent::Codex, "ordinary prompt text");

    assert_eq!(explain.state, AgentState::Idle);
    assert!(!explain.visible_idle);
    assert_eq!(
        explain.fallback_reason.as_deref(),
        Some(DEFAULT_KNOWN_AGENT_IDLE_FALLBACK)
    );
}

#[test]
fn rule_semantics_apply_gates_priority_and_line_regex() {
    with_manifest_dirs("rule-semantics", || {
        write_local_codex(&rules_manifest(
            r#"
[[rules]]
id = "low_contains"
state = "idle"
priority = 1
contains = ["match"]

[[rules]]
id = "high_nested_gates"
state = "working"
priority = 10
contains = ["match"]
all = [
  { any = [{ regex = ["w[io]n"] }, { contains = ["fallback"] }] },
]
not = [
  { contains = ["blocked"] },
]

[[rules]]
id = "line_regex"
state = "blocked"
priority = 20
line_regex = ["^exact line$"]
"#,
        ));

        let high = explain(Agent::Codex, "match win");
        assert_eq!(high.state, AgentState::Working);
        assert_eq!(
            high.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("high_nested_gates")
        );

        let not_gate = explain(Agent::Codex, "match win blocked");
        assert_eq!(not_gate.state, AgentState::Idle);
        assert_eq!(
            not_gate.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("low_contains")
        );

        let line = explain(Agent::Codex, "before\nexact line\nafter");
        assert_eq!(line.state, AgentState::Blocked);
        assert_eq!(
            line.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("line_regex")
        );
    });
}

#[test]
fn remote_manifest_loads_between_local_override_and_bundled() {
    with_manifest_dirs("remote-source", || {
        write_remote_codex(&remote_manifest("9999.01.01.1", "blocked", "remote-ready"));

        let explain = explain(Agent::Codex, "remote-ready");

        assert_eq!(explain.state, AgentState::Blocked);
        assert!(matches!(
            explain.source,
            Some(ManifestSource::Remote { .. })
        ));
        assert_eq!(explain.manifest_version.as_deref(), Some("9999.01.01.1"));
        assert_eq!(
            explain.cached_remote_version.as_deref(),
            Some("9999.01.01.1")
        );
    });
}

#[test]
fn fallback_explain_preserves_active_manifest_version() {
    with_manifest_dirs("fallback-version", || {
        write_remote_codex(&remote_manifest("9999.01.01.1", "blocked", "remote-ready"));

        let explain = explain(Agent::Codex, "ordinary prompt text");

        assert_eq!(explain.state, AgentState::Idle);
        assert_eq!(
            explain.fallback_reason.as_deref(),
            Some(DEFAULT_KNOWN_AGENT_IDLE_FALLBACK)
        );
        assert_eq!(explain.manifest_version.as_deref(), Some("9999.01.01.1"));
        assert!(matches!(
            explain.source,
            Some(ManifestSource::Remote { .. })
        ));
    });
}

#[test]
fn older_cached_remote_manifest_does_not_shadow_newer_bundled_manifest() {
    with_manifest_dirs("older-remote-bundled-fallback", || {
        write_remote_codex(&remote_manifest("2026.06.10.0", "blocked", "remote-ready"));

        let explain = explain(Agent::Codex, "remote-ready");

        assert_eq!(explain.state, AgentState::Idle);
        assert!(matches!(explain.source, Some(ManifestSource::Bundled)));
        assert_eq!(
            explain.cached_remote_version.as_deref(),
            Some("2026.06.10.0")
        );
        assert!(explain
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("older than bundled")));
    });
}

#[test]
fn local_override_shadows_cached_remote_manifest() {
    with_manifest_dirs("local-shadows-remote", || {
        write_remote_codex(&remote_manifest("9999.01.01.1", "blocked", "remote-ready"));
        write_local_codex(&local_manifest("idle", "local-ready"));

        let explain = explain(Agent::Codex, "local-ready");

        assert_eq!(explain.state, AgentState::Idle);
        assert!(matches!(explain.source, Some(ManifestSource::Override(_))));
        assert!(explain.local_override_shadowing_remote);
        assert_eq!(
            explain.cached_remote_version.as_deref(),
            Some("9999.01.01.1")
        );
    });
}

#[test]
fn invalid_local_override_falls_back_to_cached_remote_manifest() {
    with_manifest_dirs("invalid-local-remote-fallback", || {
        write_remote_codex(&remote_manifest("9999.01.01.1", "blocked", "remote-ready"));
        write_local_codex("id = ");

        let explain = explain(Agent::Codex, "remote-ready");

        assert_eq!(explain.state, AgentState::Blocked);
        assert!(matches!(
            explain.source,
            Some(ManifestSource::Remote { .. })
        ));
        assert!(explain.warning.is_some());
    });
}

#[test]
fn detection_uses_cached_manifest_until_explicit_reload() {
    with_manifest_dirs("cache-boundary", || {
        write_remote_codex(&remote_manifest("9999.01.01.1", "blocked", "cached-ready"));

        let cached = explain(Agent::Codex, "cached-ready");
        assert_eq!(cached.state, AgentState::Blocked);
        assert!(matches!(cached.source, Some(ManifestSource::Remote { .. })));
        assert_eq!(
            cached.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("test")
        );

        write_remote_codex_without_reload(&remote_manifest("9999.01.01.2", "working", "new-ready"));

        let unchanged = explain(Agent::Codex, "new-ready");
        assert_eq!(unchanged.state, AgentState::Idle);
        assert_eq!(
            unchanged.fallback_reason.as_deref(),
            Some(DEFAULT_KNOWN_AGENT_IDLE_FALLBACK)
        );
        assert_eq!(
            unchanged.cached_remote_version.as_deref(),
            Some("9999.01.01.1")
        );

        reload_manifests();

        let reloaded = explain(Agent::Codex, "new-ready");
        assert_eq!(reloaded.state, AgentState::Working);
        assert_eq!(
            reloaded.cached_remote_version.as_deref(),
            Some("9999.01.01.2")
        );
        assert_eq!(
            reloaded.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("test")
        );
    });
}

#[test]
fn all_bundled_manifests_parse_and_validate() {
    for agent in Agent::SCREEN_MANIFEST_AGENTS {
        assert!(
            bundled_manifest(agent).is_some(),
            "missing bundled manifest for {}",
            agent_label(agent)
        );
    }
}

// Compact control fragments from the named-lab TraeCode CLI 0.207.1 reads,
// rather than full-screen fixtures. Model/version/path/cost are incidental.
fn traex_context_composer(above: &str, prompt: &str, footer: &str) -> String {
    let prompt_line = if prompt.is_empty() {
        "❯".to_string()
    } else {
        format!("❯ {prompt}")
    };
    format!("{above}\n───────────────────────── lab ─\n{prompt_line}\n───────────────────────────────\n  {footer}\n")
}

fn traex_bordered_composer(above: &str, prompt: &str, footer: &str, width: usize) -> String {
    let prompt_line = if prompt.is_empty() {
        "❯".to_string()
    } else {
        format!("❯ {prompt}")
    };
    let border = "─".repeat(width);
    let footer = if footer.is_empty() {
        String::new()
    } else {
        format!("\n  {footer}")
    };
    format!("{above}\n{border}\n{prompt_line}\n{border}{footer}\n")
}

fn traex_halfblock_composer(above: &str) -> String {
    format!(
        "{above}\n\n❯ Use /skills to list available skills\n{}",
        "▀".repeat(219)
    )
}

fn traex_halfblock_composer_with_footer(above: &str, footer: &str) -> String {
    format!("{}\n  {footer}", traex_halfblock_composer(above))
}

#[test]
fn traex_halfblock_composer_replays_recorded_geometry() {
    let screen = traex_halfblock_composer("服务保持运行…");
    let result = explain(Agent::Traex, &screen);

    assert_eq!(screen.lines().next_back().unwrap().chars().count(), 219);
    assert_eq!(result.state, AgentState::Idle, "{screen}");
    assert_eq!(
        result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("observed_screen_idle")
    );
    assert!(result.visible_idle);
}

#[test]
fn traex_halfblock_composer_preserves_active_and_blocked_precedence() {
    let working = traex_halfblock_composer(
        "◆ Working… (5s • esc to interrupt) · 1 shell running… · /ps to manage",
    );
    let without_osc = explain(Agent::Traex, &working);
    assert_eq!(without_osc.state, AgentState::Working, "{working}");
    assert_eq!(
        without_osc
            .matched_rule
            .as_ref()
            .map(|rule| rule.id.as_str()),
        Some("current_interrupt_working")
    );
    let with_osc = super::super::detect_agent_with_osc(
        Some(Agent::Traex),
        &traex_halfblock_composer(""),
        "⠋ lab",
        "",
    );
    assert_eq!(with_osc.state, AgentState::Working);

    for panel in [
        "─────────────────────\n  Would you like to run the following command?\n  $ printf HERDR_PERMISSION_PROBE\n❯ 1. Yes, proceed (y)\n  5. No, and tell TraeCode CLI what to do differently (esc)\n  enter confirm  |  esc cancel",
        "─────────────────────\n  Question 1/1 (1 unanswered)\n  Should the probe color be red or blue?\n  ❯ 1. Red (Recommended)\n    2. Blue\n  tab add notes  |  enter submit answer  |  esc interrupt",
    ] {
        for blocked in [
            traex_halfblock_composer(panel),
            traex_halfblock_composer_with_footer(
                panel,
                "changed model · wrapped path\n  permission footer changed · ← for agents",
            ),
        ] {
            let result = explain(Agent::Traex, &blocked);
            assert_eq!(result.state, AgentState::Blocked, "{blocked}");
            assert!(result.visible_blocker, "{blocked}");
        }
    }

    let unknown_activity = traex_halfblock_composer("◆ Future activity… (5s • esc to interrupt)");
    let result = explain(Agent::Traex, &unknown_activity);
    assert_eq!(result.state, AgentState::Working, "{unknown_activity}");
    assert!(result.visible_working);

    for status in [
        "◆ Future activity… (5s • esc to interrupt)",
        "✦ 状态文字换行\n  继续执行 (6s • esc to interrupt) · 1 shell running…",
    ] {
        let active_hybrid = traex_halfblock_composer_with_footer(
            status,
            "changed model · wrapped path\n  permission footer changed · ← for agents",
        );
        let result = explain(Agent::Traex, &active_hybrid);
        assert_eq!(result.state, AgentState::Working, "{active_hybrid}");
        assert!(result.visible_working, "{active_hybrid}");
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("current_interrupt_working"),
            "{active_hybrid}"
        );
    }

    let typed_prompt = format!("❯ Keep working\n{}", "▀".repeat(219));
    let result = explain(Agent::Traex, &typed_prompt);
    assert_eq!(result.state, AgentState::Idle, "{typed_prompt}");
    assert!(result.visible_idle);
}

#[test]
fn traex_loaded_context_composer_accepts_captured_truncated_footer() {
    let screen = traex_context_composer(
        "◆ 你好，宇航！今天想一起处理什么？",
        "hello",
        "GPT-5.6-Sol high · Context 95% … ▧ Workspace Edit",
    );
    let result = explain(Agent::Traex, &screen);

    assert_eq!(result.state, AgentState::Idle, "{screen}");
    assert!(result.visible_idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("observed_screen_idle")
    );
}

#[test]
fn traex_loaded_context_composer_is_not_required_for_live_idle() {
    for (prompt, footer) in [
        (
            "Find and fix a bug in @filename",
            "GPT-5.6-Sol high · Context 100% left · ⎇ herdr · /work…",
        ),
        ("", "another model low · Context 93% left · $0.000"),
        (
            "a long input\nwrapped onto the next line",
            "GPT-5.6-Sol high · Context 92% left · ⎇ h…",
        ),
        ("typed but not submitted", "new unknown footer"),
        ("", ""),
    ] {
        let screen = traex_context_composer("◆ Completed", prompt, footer);
        let result = super::super::detect_agent(Some(Agent::Traex), &screen);
        assert_eq!(result.state, AgentState::Idle, "{screen}");
        assert!(result.visible_idle);
        assert_eq!(
            super::super::detect_agent(None, &screen).state,
            AgentState::Unknown
        );
    }

    let footer = "GPT-5.6-Sol high · Context 100% left · ⎇ herdr";
    let ready = traex_context_composer("", "Find a bug", footer);
    for screen in [
        footer.to_string(),
        format!("❯ Find a bug\n  {footer}"),
        format!("────────────────\n  {footer}"),
        ready.replace("❯ Find a bug\n", ""),
        format!("{ready}user@host:~$ "),
    ] {
        let result = super::super::detect_agent(Some(Agent::Traex), &screen);
        assert_eq!(result.state, AgentState::Idle, "{screen}");
        assert!(result.visible_idle);
    }

    // Historical activity/approval text must not override the live composer.
    let stale = traex_context_composer(
        "◈ Working… (2s • esc to interrupt)\nWould you like to run the following command?\nYes, proceed\nenter confirm | esc cancel\n◆ Completed",
        "Find a bug",
        footer,
    );
    assert_eq!(
        super::super::detect_agent(Some(Agent::Traex), &stale).state,
        AgentState::Idle
    );
}

#[test]
fn traex_current_interrupt_control_is_label_independent() {
    for status in [
        "◈ Working… (4s • esc to interrupt) · 1 shell running… · /ps to manage",
        "◇ Future phase 47 (6s • esc to interrupt)",
        "❖ 等待外部服务 (2s • esc to interrupt)",
        "✦ Esperando herramienta (3s • esc to interrupt)",
        "◆ Überprüfung läuft (5s • esc to interrupt)",
        "✧ 新しい処理状態 (9s • esc to interrupt)",
        "◆\tPhase-with-punctuation: I/O? (10s • esc to interrupt) · new control",
        "◆   Unknown activity (10s • esc to interrupt) · 1 shell running… · /ps to manage",
        "◆ Label wraps before the\n  control line (10s • esc to interrupt) · extra control\n  wrapped continuation",
        "◆ 状态文字也可能换行\n  并继续 (5s • esc   to\ninterrupt) · 1 shell running… · /ps to\n  manage",
    ] {
        let screen = traex_context_composer(
            status,
            "Find a bug",
            "GPT-5.6-Sol high · Context 93% left · /work…",
        );
        // No OSC available: exercise the actual rendered fallback.
        let result = explain(Agent::Traex, &screen);
        assert_eq!(result.state, AgentState::Working, "{screen}");
        assert!(result.visible_working);
        assert!(!result.visible_idle);
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("current_interrupt_working"),
            "{screen}"
        );
        assert_eq!(
            super::super::detect_agent(None, &screen).state,
            AgentState::Unknown,
            "an unidentified process must not inherit TraeX activity: {screen}"
        );
    }

    let captured_truncated = traex_context_composer(
        "◆ Working… (5s • esc to interrupt)",
        "hello",
        "GPT-5.6-Sol high · Context 95% … ▧ Workspace Edit",
    );
    let result = super::super::detect_agent(Some(Agent::Traex), &captured_truncated);
    assert_eq!(result.state, AgentState::Working, "{captured_truncated}");
    assert!(result.visible_working);
    assert!(!result.visible_idle);

    let ready = traex_context_composer("", "Find a bug", "model · Context 93% left");
    assert_eq!(
        super::super::detect_agent_with_osc(Some(Agent::Traex), &ready, "⠋ lab", "").state,
        AgentState::Working
    );
    // Static titles are not sufficient. The current interrupt control, rather
    // than any catalogue of status labels, is the visible activity authority.
    assert_eq!(
        osc_explain(Agent::Traex, "", "herdr", "").state,
        AgentState::Unknown
    );
}

#[test]
fn traex_observed_idle_is_footer_and_layout_independent() {
    for (width, prompt, footer) in [
        (20, "", ""),
        (38, "typed but not submitted", "new model · unknown footer"),
        (
            120,
            "first line\nwrapped second line",
            "permission text changed again",
        ),
    ] {
        let screen = traex_bordered_composer("◆ Completed", prompt, footer, width);
        let result = explain(Agent::Traex, &screen);
        assert_eq!(result.state, AgentState::Idle, "{screen}");
        assert!(result.visible_idle, "{screen}");
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("observed_screen_idle")
        );
    }

    let typed_halfblock = format!(
        "◆ Completed\n\n❯ typed but not submitted\n{}",
        "▀".repeat(37)
    );
    let result = explain(Agent::Traex, &typed_halfblock);
    assert_eq!(result.state, AgentState::Idle, "{typed_halfblock}");
    assert!(result.visible_idle);

    for unreadable in ["", "   \n\t"] {
        let result = explain(Agent::Traex, unreadable);
        assert_eq!(result.state, AgentState::Unknown, "{unreadable:?}");
        assert!(!result.visible_idle);
    }
}

#[test]
fn traex_live_screen_without_current_activity_is_idle() {
    let captured_tail = format!(
        "{} Define herdr agent instructions ▄\n❯ Explain this codebase\n{}\n  GPT-5.6-Luna medium · Context 63% left · ⎇ herdr · /data00/home/chengyuhang/.treehouse/herdr-8a0084/1/herdr · No committed line changes                              ☢ Full Access (shift+tab to cycle) · ← for agents\n",
        "▄".repeat(219),
        "▀".repeat(219),
    );

    for screen in [
        captured_tail,
        "◆ Completed\n❯ next\n▀▀▀▀▀▀▀▀▀▀▀\n  a completely different footer".to_string(),
        "◆ Completed\n❯ next\n▀▀▀▀▀▀▀▀▀▀▀".to_string(),
        "◆ Completed\n❯ next\n▀▀▀▀▀▀▀▀▀▀▀\n  model · a wrapped footer\n  path · permission changed"
            .to_string(),
        "◆ Completed\ncurrent composer chrome changed".to_string(),
        "historic ◈ Working… (20s • esc to interrupt)\n◆ Completed\ncurrent transcript".to_string(),
    ] {
        let result = explain(Agent::Traex, &screen);
        assert_eq!(result.state, AgentState::Idle, "{screen}");
        assert!(result.visible_idle, "{screen}");
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("observed_screen_idle"),
            "{screen}"
        );
    }

    let unidentified = super::super::detect_agent(None, "◆ Completed\ncurrent transcript");
    assert_eq!(unidentified.state, AgentState::Unknown);
    assert!(!unidentified.visible_idle);
}

#[test]
fn traex_historic_activity_and_waits_do_not_override_current_composer() {
    for historic in [
        "◆ Waiting for command (20s • esc to interrupt) · 1 shell running… · /ps to manage\nWould you like to run the following command?\nYes, proceed\nenter confirm | esc cancel\n◆ Completed",
        "◆ ◆ Waiting for command (20s • esc to interrupt) · 1 shell running… · /ps to manage\n  Would you like to run the following command? Yes, proceed enter confirm | esc cancel",
        "✦ Esperando herramienta (20s • esc to interrupt) · future controls\n◆ Completed",
    ] {
        let screen = traex_bordered_composer(historic, "next request", "anything", 52);
        let result = explain(Agent::Traex, &screen);
        assert_eq!(result.state, AgentState::Idle, "{screen}");
        assert!(result.visible_idle);
    }

    let stale_hybrid = traex_halfblock_composer_with_footer(
        "◆ Working… (2s • esc to interrupt)\nordinary completed response",
        "changed model · path · permission",
    );
    let result = explain(Agent::Traex, &stale_hybrid);
    assert_eq!(result.state, AgentState::Idle, "{stale_hybrid}");
    assert!(result.visible_idle, "{stale_hybrid}");
    assert_eq!(
        result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("observed_screen_idle")
    );
}

#[test]
fn traex_loaded_approval_and_question_panels_block_without_osc() {
    for panel in [
        "─────────────────────\n  Would you like to run the following command?\n  $ printf HERDR_PERMISSION_PROBE\n❯ 1. Yes, proceed (y)\n  5. No, and tell TraeCode CLI what to do differently (esc)\n  enter confirm  |  esc cancel\n",
        "─────────────────────\n  Question 1/1 (1 unanswered)\n  Should the probe color be red or blue?\n  ❯ 1. Red (Recommended)\n    2. Blue\n  tab add notes  |  enter submit answer  |  esc interrupt\n",
        "─────────────────────\n  Would you like to run the following\n  command?\n❯ 1. Yes, proceed (y)\n  enter confirm | esc cancel\n",
        "─────────────────────\n  Question 1/1 (1 unanswered)\n  ❯ 1. Blue (Recommended)\n    2. Red\n  tab add notes | enter submit answer\n  esc interrupt\n",
    ] {
        let result = super::super::detect_agent(Some(Agent::Traex), panel);
        assert_eq!(result.state, AgentState::Blocked);
        assert!(result.visible_blocker);
        let screen = format!("{panel}{}", traex_context_composer("◆ Completed", "", "model · Context 93% left"));
        assert_eq!(super::super::detect_agent(Some(Agent::Traex), &screen).state, AgentState::Idle);
    }
}

#[test]
fn traex_manifest_matches_only_captured_terminal_states() {
    let idle = osc_explain(
        Agent::Traex,
        "TRAE CLI Next (v0.200.19)\n────────────────────────\n❯ Use /skills to list available skills\n────────────────────────\n  GPT-5.6-Sol m… ▰ Full Access (shift+tab to cycle)",
        "tmp",
        "",
    );
    assert_eq!(idle.state, AgentState::Idle);
    assert_eq!(
        idle.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("observed_screen_idle")
    );
    assert!(idle.visible_idle);

    let workspace_edit_idle = explain(
        Agent::Traex,
        "TRAE CLI Next (v0.200.19)\n────────────────────────\n❯ Improve documentation in @filename\n──────────────────────────────────────────────────────\n  GPT-5.6-Sol… ◐ Workspace Edit (shift+tab to cycle)",
    );
    assert_eq!(workspace_edit_idle.state, AgentState::Idle);
    assert_eq!(
        workspace_edit_idle
            .matched_rule
            .as_ref()
            .map(|rule| rule.id.as_str()),
        Some("observed_screen_idle")
    );
    assert!(workspace_edit_idle.visible_idle);

    for footer in [
        "GPT-5.6-Sol m… ▰ Full Access",
        "GPT-5.6-Sol… ◐ Workspace Edit",
    ] {
        let no_hint = explain(
            Agent::Traex,
            &format!(
                "TRAE CLI Next (v0.200.19)\n────────────────────────\n❯ Use /skills to list available skills\n────────────────────────\n  {footer}"
            ),
        );
        assert_eq!(no_hint.state, AgentState::Idle);
        assert_eq!(
            no_hint.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("observed_screen_idle")
        );
        assert!(no_hint.visible_idle);
    }

    let osc_working = osc_explain(
        Agent::Traex,
        "❯ Use /skills to list available skills\n  GPT-5.6-Sol m… ▰ Full Access (shift+tab to cycle)",
        "⠋ tmp",
        "",
    );
    assert_eq!(osc_working.state, AgentState::Working);
    assert_eq!(
        osc_working
            .matched_rule
            .as_ref()
            .map(|rule| rule.id.as_str()),
        Some("osc_spinner_working")
    );
    assert!(osc_working.visible_working);

    for status_line in [
        "◈ Working… (2s • esc to interrupt)",
        "◇ Working… (6s • esc to interrupt) · 1 shell running…",
    ] {
        let visible_working = explain(
            Agent::Traex,
            &traex_context_composer(
                status_line,
                "",
                "GPT-5.6-Sol m… ▰ Full Access (shift+tab to cycle)",
            ),
        );
        assert_eq!(visible_working.state, AgentState::Working);
        assert_eq!(
            visible_working
                .matched_rule
                .as_ref()
                .map(|rule| rule.id.as_str()),
            Some("current_interrupt_working")
        );
    }

    for trust_prompt in [
        "Do you trust the contents of this directory?\n❯ 1. Yes, continue\n  2. No, quit\nPress enter to continue",
        "Folder access\n/task/fixture\nDo you trust the contents of this directory? Working with untrusted contents comes with higher risk of prompt injection.\n❯ 1. Yes, continue\n  2. No, quit\nenter continue  |  esc quit",
    ] {
        let trust = explain(Agent::Traex, trust_prompt);
        assert_eq!(trust.state, AgentState::Blocked, "{trust_prompt}");
        assert_eq!(
            trust.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("directory_trust_prompt")
        );
        assert!(trust.visible_blocker);
    }

    let alias_migration = explain(
        Agent::Traex,
        "TRAE CLI Next is installed.\nDo you want to point Coco command aliases to TRAE\nCLI Next?\n❯ Switch aliases\n  Keep Coco",
    );
    assert_eq!(alias_migration.state, AgentState::Blocked);
    assert_eq!(
        alias_migration
            .matched_rule
            .as_ref()
            .map(|rule| rule.id.as_str()),
        Some("alias_migration_prompt")
    );

    let observed = explain(Agent::Traex, "an observed Traex surface");
    assert_eq!(observed.state, AgentState::Idle);
    assert_eq!(
        observed.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("observed_screen_idle")
    );
    assert_eq!(observed.fallback_reason, None);
    assert!(observed.visible_idle);
    assert!(!observed.visible_working);
    assert!(!observed.visible_blocker);
}

#[test]
fn devin_manifest_detects_idle_working_and_blocked_states() {
    let idle = explain(
        Agent::Devin,
        "─────────────────────────────────────────────────────\n❭ Ask Devin to build features, fix bugs, or work on\n  your code\n─────────────────────────────────────────────────────\nSWE-1.6               Context: 16k / 200k tokens (7%)",
    );
    assert_eq!(idle.state, AgentState::Idle);
    assert!(idle.visible_idle);

    let live_footer_idle = explain(
        Agent::Devin,
        "Done.\n\n────────────────────────────────────────────────── (bypass permissions on) ─\n❭\n────────────────────────────────────────────────────────────────────────────\nClaude Opus 4.6 Thinking                                    Context: 38k / 200k tokens (18%)",
    );
    assert_eq!(live_footer_idle.state, AgentState::Idle);
    assert_eq!(
        live_footer_idle
            .matched_rule
            .as_ref()
            .map(|rule| rule.id.as_str()),
        Some("live_prompt_footer")
    );
    assert!(live_footer_idle.visible_idle);

    let welcome_footer_idle = explain(
        Agent::Devin,
        "⠀⠀⠀⠀⠀⣴⣾⣶⡄⠀⠀⠀⠀\n⠀⣴⣾⣶⡾⠛⠿⠟⠃⣴⣾⣶⡄  Devin CLI\n⠀⠛⠿⠟⠃⣴⣾⣶⡾⠛⠿⠟⠃  v2026.5.26-8\n⠀⣤⣶⣦⡄⠻⢿⠿⢷⣤⣶⣦⡄\n⠀⠻⢿⠿⢷⣤⣶⣦⡄⠻⢿⠿⠃  Hybrid\n⠀⠀⠀⠀⠀⠻⢿⠿⠃⠀⠀⠀⠀\n\n───────────────────────────\n❭ Ask Devin to build\n  features, fix bugs, or\n  work on your code\n───────────────────────────\nClaude Opus Looking for\n4.6 Thinkingplan mode? /\n            plan",
    );
    assert_eq!(welcome_footer_idle.state, AgentState::Idle);
    assert_eq!(
        welcome_footer_idle
            .matched_rule
            .as_ref()
            .map(|rule| rule.id.as_str()),
        Some("welcome_prompt_footer")
    );
    assert!(welcome_footer_idle.visible_idle);

    let working = explain(
        Agent::Devin,
        "◔ Reading shell 91b655\n  │ Timeout: 35s\n\n⠀⡆ Running tools · 27s (esc to interrupt)\n─────────────────────────────────────────────────────\n❭ Guide Devin while it works",
    );
    assert_eq!(working.state, AgentState::Working);
    assert!(working.visible_working);

    let trust_prompt = explain(
        Agent::Devin,
        "Do you trust the authors of this directory?\nFor security, devin should not be run in directories\nwith untrusted content.\n❭ 1 Yes, trust /private/tmp/devin-hook-probe\n· 2 No, exit",
    );
    assert_eq!(trust_prompt.state, AgentState::Blocked);
    assert!(trust_prompt.visible_blocker);

    let permission_prompt = explain(
        Agent::Devin,
        "⏺ Running command\n  └ $ sleep 30\n\n❭ 1 Yes  (Approve once)\n· 2 Yes, allow `sleep` commands\n· 3 Yes, always allow `sleep` commands\n· 4 No\n↑↓ select · ↵ confirm · esc cancel",
    );
    assert_eq!(permission_prompt.state, AgentState::Blocked);
    assert!(permission_prompt.visible_blocker);
}

#[test]
fn manifest_validation_rejects_unknown_fields_empty_rules_invalid_regions_and_regexes() {
    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "typo"
state = "working"
contain = ["Working"]
"#
    )
    .is_err());

    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "empty"
state = "working"
"#
    )
    .is_err());

    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "bad_region"
state = "working"
region = "after_last_promt_marker"
contains = ["Working"]
"#
    )
    .is_err());

    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "bad_regex"
state = "working"
regex = ["["]
"#
    )
    .is_err());

    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "bad_nested_regex"
state = "working"
any = [{ line_regex = ["["] }]
"#
    )
    .is_err());
}

#[test]
fn manifest_validation_keeps_skip_rules_neutral() {
    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "bad_skip_state"
state = "idle"
skip_state_update = true
contains = ["menu"]
"#
    )
    .is_err());

    assert!(parse_manifest(
        r#"
id = "codex"

[[rules]]
id = "bad_skip_visible"
state = "unknown"
skip_state_update = true
visible_blocker = true
contains = ["menu"]
"#
    )
    .is_err());
}

#[test]
fn manifest_validation_rejects_excessive_rule_count() {
    let mut manifest = String::from(
        r#"
id = "codex"
"#,
    );
    for index in 0..129 {
        manifest.push_str(&format!(
            r#"
[[rules]]
id = "rule_{index}"
state = "idle"
contains = ["ready"]
"#
        ));
    }

    assert!(parse_manifest(&manifest).is_err());
}

#[test]
fn manifest_validation_rejects_excessive_gate_depth() {
    let manifest = r#"
id = "codex"

[[rules]]
id = "deep"
state = "idle"
contains = ["ready"]
all = [
  { contains = ["1"], all = [
    { contains = ["2"], all = [
      { contains = ["3"], all = [
        { contains = ["4"], all = [
          { contains = ["5"], all = [
            { contains = ["6"], all = [
              { contains = ["7"], all = [
                { contains = ["8"], all = [
                  { contains = ["9"] },
                ] },
              ] },
            ] },
          ] },
        ] },
      ] },
    ] },
  ] },
]
"#;

    assert!(parse_manifest(manifest).is_err());
}

#[test]
fn manifest_validation_rejects_excessive_matchers() {
    let matchers = (0..33)
        .map(|index| format!(r#""m{index}""#))
        .collect::<Vec<_>>()
        .join(", ");
    let manifest = format!(
        r#"
id = "codex"

[[rules]]
id = "many"
state = "idle"
contains = [{matchers}]
"#
    );

    assert!(parse_manifest(&manifest).is_err());
}

#[test]
fn bottom_non_empty_lines_uses_bottom_occurrence_for_repeated_text() {
    let content = "marker\nold\n\nmiddle\nmarker\nnew\n";

    assert_eq!(
        region(
            DetectionInput {
                screen: content,
                osc_title: "",
                osc_progress: "",
            },
            "bottom_non_empty_lines(2)"
        ),
        "marker\nnew\n"
    );
}

#[test]
fn top_non_empty_lines_uses_top_occurrence_for_repeated_text() {
    let content = "\nmarker\nold\n\nmiddle\nmarker\nnew\n";

    assert_eq!(
        region(
            DetectionInput {
                screen: content,
                osc_title: "",
                osc_progress: "",
            },
            "top_non_empty_lines(2)"
        ),
        "\nmarker\nold\n"
    );
}

#[test]
fn top_non_empty_lines_requires_a_canonical_positive_bounded_count() {
    let name = "top_non_empty_lines";
    assert!(validate_region_name(&format!("{name}(1)")).is_ok());
    assert!(validate_region_name(&format!("{name}({})", u16::MAX)).is_ok());
    for count in ["0", "01", "+1", "65536", "999999999999999999999999"] {
        assert!(
            validate_region_name(&format!("{name}({count})")).is_err(),
            "{name} accepted invalid count {count}"
        );
    }
}

#[test]
fn top_non_empty_lines_requires_engine_three_when_declared() {
    let manifest = r#"
id = "grok"
version = "1"
min_engine_version = 2

[[rules]]
id = "background"
state = "working"
region = " top_non_empty_lines(1) "
contains = ["active"]
"#;

    assert!(parse_manifest(manifest).is_err());
}

// ---------------------------------------------------------------------------
// OSC rule tests — exercise the new osc_title / osc_progress regions against
// the bundled Claude and Codex manifests.
// ---------------------------------------------------------------------------

fn osc_explain(
    agent: Agent,
    screen: &str,
    osc_title: &str,
    osc_progress: &str,
) -> DetectionExplain {
    explain_with_input(
        agent,
        DetectionInput {
            screen,
            osc_title,
            osc_progress,
        },
    )
}

// --- Claude OSC rules ---

#[test]
fn claude_osc_title_braille_prefix_is_working() {
    // "⠂" is U+2802, in the braille block U+2800-U+28FF
    let result = osc_explain(Agent::Claude, "", "⠂ project", "");
    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_working")
    );
    assert!(result.visible_working);
}

#[test]
fn claude_osc_title_static_prefix_is_idle() {
    // "✳" is U+2733, static prefix when Claude is not working
    let result = osc_explain(Agent::Claude, "", "✳ Claude Code", "");
    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_idle")
    );
    assert!(result.visible_idle);
}

#[test]
fn claude_osc_progress_4_3_alone_does_not_force_working() {
    // Claude leaves progress stuck at 4;3 while waiting for permission, so
    // 4;3 must not be a working signal on its own. With no other evidence it
    // falls back to idle; blocked screen rules can win when present.
    let result = osc_explain(Agent::Claude, "", "", "4;3;");
    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.fallback_reason.as_deref(),
        Some(DEFAULT_KNOWN_AGENT_IDLE_FALLBACK)
    );
    assert!(!result.visible_working);
}

#[test]
fn claude_blocker_screen_outranks_stale_osc_progress() {
    // Regression: progress 4;3 persists during permission prompts. The
    // blocked form on screen must win because no rule treats 4;3 as working.
    let blocker_screen =
        "──────────\n  1. Yes\n  2. No\n\nEnter to select · ↑/↓ to navigate · Esc to cancel\n";
    let result = osc_explain(Agent::Claude, blocker_screen, "✳ Task title", "4;3;");
    assert_eq!(result.state, AgentState::Blocked);
    assert!(result.visible_blocker);
}

#[test]
fn claude_osc_progress_4_0_is_idle() {
    let result = osc_explain(Agent::Claude, "", "", "4;0;");
    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_progress_idle")
    );
}

#[test]
fn claude_blocker_screen_outranks_osc_idle_title() {
    // When the OSC title shows ✳ (idle) but the screen has a bash permission
    // prompt, the blocked rule at priority 850 beats osc_title_idle at 250.
    let blocker_screen = "do you want to proceed?\n\
        bash command: rm -rf /tmp/test\n\
        ❯ 1. Yes\n   2. No\n\n\
        Esc to cancel · Tab to amend · ctrl+e to explain\n";
    let result = osc_explain(Agent::Claude, blocker_screen, "✳ Claude Code", "");
    assert_eq!(result.state, AgentState::Blocked);
    assert!(result.visible_blocker);
}

#[test]
fn claude_empty_osc_empty_screen_is_idle_fallback() {
    // No OSC data, no matching screen rule → fallback idle (unchanged V3 behavior)
    let result = osc_explain(Agent::Claude, "", "", "");
    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.fallback_reason.as_deref(),
        Some(DEFAULT_KNOWN_AGENT_IDLE_FALLBACK)
    );
    assert!(!result.visible_idle);
}

// --- Codex OSC rules ---

#[test]
fn codex_osc_title_braille_spinner_is_working() {
    // "⠋" is U+280B, in the braille block
    let result = osc_explain(Agent::Codex, "", "⠋ llm-proxy", "");
    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_working")
    );
    assert!(result.visible_working);
}

#[test]
fn codex_osc_title_action_required_is_blocked() {
    let result = osc_explain(Agent::Codex, "", "[ . ] Action Required | llm-proxy", "");
    assert_eq!(result.state, AgentState::Blocked);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_blocked")
    );
    assert!(result.visible_blocker);
}

#[test]
fn codex_osc_title_plain_is_idle() {
    let result = osc_explain(Agent::Codex, "", "llm-proxy", "");
    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_idle")
    );
    assert!(result.visible_idle);
}

#[test]
fn codex_background_terminal_screen_does_not_override_osc_idle() {
    // Background terminal tasks can be long-lived helpers such as dev servers.
    // They should not make Codex look busy once the foreground turn is idle.
    let screen = "background terminal running · /ps to view · /stop to close\n";
    let result = osc_explain(Agent::Codex, screen, "llm-proxy", "");
    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_idle")
    );
    assert!(result.visible_idle);
}

#[test]
fn codex_screen_working_fallback_handles_static_osc_title() {
    let screen = "• I’ll run it and wait for completion.\n\n\
        ◦ Working (1m 16s • esc to interrupt) · 1 background…\n\n\
        › Use /skills to list available skills\n\n\
        gpt-5.6-sol default · /work\n";
    let result = osc_explain(Agent::Codex, screen, "project", "");

    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("screen_working_fallback")
    );
    assert!(result.visible_working);
}

#[test]
fn codex_osc_working_remains_preferred_over_screen_fallback() {
    let screen = "• Working (4s • esc to interrupt)\n\n\
        › Use /skills to list available skills\n\n\
        gpt-5.6-sol default · /work\n";
    let result = osc_explain(Agent::Codex, screen, "⠸ project", "");

    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_working")
    );
    assert!(result.visible_working);
}

#[test]
fn codex_screen_blocker_outranks_working_fallback() {
    let screen = "• Working (4s • esc to interrupt)\n\
        › 1. Yes, proceed\n\
        Press enter to confirm or esc to cancel\n";
    let result = osc_explain(Agent::Codex, screen, "project", "");

    assert_eq!(result.state, AgentState::Blocked);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("live_strong_blocker")
    );
    assert!(result.visible_blocker);
    assert!(!result.visible_working);
}

#[test]
fn codex_weak_blocker_outranks_working_fallback() {
    let screen = "• Working (4s • esc to interrupt)\n\
        do you want to continue? [y/n]\n\
        › Use /skills to list available skills\n";
    let result = osc_explain(Agent::Codex, screen, "project", "");

    assert_eq!(result.state, AgentState::Blocked);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("weak_blocker")
    );
    assert!(!result.visible_working);
}

#[test]
fn codex_transcript_viewer_outranks_working_fallback() {
    let screen = "• Working (4s • esc to interrupt)\n\
        › transcript\n\
        ↑/↓ to scroll · pgup/pgdn to move · home/end to jump · q to quit · esc to edit prev\n";
    let result = osc_explain(Agent::Codex, screen, "project", "");

    assert_eq!(result.state, AgentState::Unknown);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("transcript_viewer")
    );
    assert!(result.skip_state_update);
    assert!(!result.visible_working);
}

#[test]
fn codex_screen_working_fallback_ignores_stale_and_prompt_text() {
    let screens = [
        "◦ Working (1m 16s • esc to interrupt)\n\
         ■ Conversation interrupted\n\
         › Use /skills to list available skills\n\
         gpt-5.6-sol default · /work\n",
        "› Explain the text ◦ Working (1m 16s • esc to interrupt)\n\
         gpt-5.6-sol default · /work\n",
        "  ◦ Working (1m 16s • esc to interrupt)\n\
         › Use /skills to list available skills\n\
         gpt-5.6-sol default · /work\n",
    ];

    for screen in screens {
        let result = osc_explain(Agent::Codex, screen, "project", "");
        assert_eq!(result.state, AgentState::Idle);
        assert_eq!(
            result.matched_rule.as_ref().map(|r| r.id.as_str()),
            Some("osc_title_idle")
        );
        assert!(result.visible_idle);
        assert!(!result.visible_working);
    }
}

#[test]
fn codex_screen_working_fallback_ignores_interrupted_short_terminal() {
    let screen = "◦ Working (1m 16s • esc to interrupt)\n\
        ■ Conversation interrupted\n\
        ›\n";
    let result = osc_explain(Agent::Codex, screen, "project", "");

    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_idle")
    );
    assert!(result.visible_idle);
    assert!(!result.visible_working);
}

#[test]
fn codex_osc_working_beats_weak_blocker_screen() {
    // A stale [y/n] on screen triggers weak_blocker at priority 600, but an
    // active braille spinner in the OSC title is priority 1050 — OSC wins.
    let screen = "do you want to continue? [y/n]\n";
    let result = osc_explain(Agent::Codex, screen, "⠋ llm-proxy", "");
    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_working")
    );
}
