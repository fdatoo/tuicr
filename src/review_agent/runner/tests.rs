use super::*;

#[test]
fn claude_stream_returns_only_final_answer_without_duplicate_delta() {
    // Reduced from Claude 2.1.326 stream-json: init, a tool-use content block,
    // two text deltas, the full assistant message and the final result.
    let frames = [
        r#"{"type":"system","subtype":"init","tools":["Glob","Grep","Read"]}"#,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","name":"Read","input":{}}}}"#,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"tangerine "}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"obsidian"}}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"tangerine obsidian"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"tangerine obsidian"}"#,
    ];
    let mut parser = Stream::default();
    let mut activities = Vec::new();
    let mut text = String::new();
    for frame in frames {
        assert!(
            parser
                .event(
                    Provider::Claude,
                    frame.as_bytes(),
                    &mut |event| match event {
                        AgentEvent::Activity(activity) => activities.push(activity.to_owned()),
                        AgentEvent::Text(delta) => text.push_str(delta),
                    }
                )
                .unwrap()
        );
    }
    assert_eq!(activities, ["Reviewer started", "Reading source"]);
    assert_eq!(text, "tangerine obsidian");
    assert_eq!(
        parser.finish(Provider::Claude).unwrap().as_deref(),
        Some(text.as_str())
    );
}

#[test]
fn codex_stream_completes_after_warning_without_tools() {
    // Reduced from Codex 0.167.2: a model requiring Code Mode warns about
    // the disabled host, but can still answer from what was sent on stdin.
    let frames = [
        r#"{"type":"thread.started","thread_id":"example"}"#,
        r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Code Mode is unavailable because code-mode host is disabled"}}"#,
        r#"{"type":"turn.started"}"#,
        r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"Evidence came from the diff."}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":15,"output_tokens":10}}"#,
    ];
    let mut parser = Stream::default();
    let mut text = String::new();
    for frame in frames {
        parser
            .event(Provider::Codex, frame.as_bytes(), &mut |event| {
                if let AgentEvent::Text(delta) = event {
                    text.push_str(delta);
                }
            })
            .unwrap();
    }
    assert_eq!(text, "Evidence came from the diff.");
    assert_eq!(
        parser.finish(Provider::Codex).unwrap().as_deref(),
        Some(text.as_str())
    );
}

#[test]
fn provider_errors_never_quote_private_output() {
    let mut parser = Stream::default();
    for (provider, frame, login) in [
        (
            Provider::Claude,
            r#"{"type":"result","subtype":"error","result":"private"}"#,
            "/login",
        ),
        (
            Provider::Codex,
            r#"{"type":"turn.failed","error":{"message":"private"}}"#,
            "codex login",
        ),
    ] {
        let err = parser
            .event(provider, frame.as_bytes(), &mut |_| {})
            .unwrap_err();
        assert!(err.contains(login));
        assert!(!err.contains("private"));
    }
    assert!(
        !parser
            .event(Provider::Claude, b"private invalid json", &mut |_| {})
            .unwrap_err()
            .contains("private")
    );
    assert!(
        parser
            .emit(&"x".repeat(MAX_ANSWER_BYTES + 1), &mut |_| unreachable!())
            .is_err()
    );
}

#[test]
fn review_provider_keeps_model_in_one_argument_and_disables_unsafe_tools() {
    for provider in [Provider::Claude, Provider::Codex] {
        let request = AgentRequest {
            provider,
            model: Some("--dangerously-bypass-approvals-and-sandbox two".into()),
            thorough: true,
            dir: PathBuf::from("snapshot"),
            prompt: "private prompt".into(),
        };
        let command = request.command();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert!(args.contains(&"--model=--dangerously-bypass-approvals-and-sandbox two"));
        assert!(!args.iter().any(|arg| arg.contains("private prompt")));
        assert_eq!(command.get_current_dir(), Some(request.dir.as_path()));
        match provider {
            Provider::Claude => {
                assert!(args.windows(2).any(|a| a == ["--tools", "Read,Glob,Grep"]));
                assert!(args.windows(2).any(|a| a == ["--effort", "high"]));
                assert!(args.contains(&"--restricted"));
            }
            Provider::Codex => {
                assert!(args.windows(2).any(|a| a == ["--disable", "shell_tool"]));
                assert!(
                    args.windows(2)
                        .any(|a| a == ["--disable", "code_mode_host"])
                );
                assert!(args.contains(&"model_reasoning_effort=\"medium\""));
                assert!(args.contains(&"mcp_servers={}"));
            }
        }
    }
}

#[test]
fn missing_review_provider_has_actionable_error() {
    let temp = tempfile::tempdir().unwrap();
    let mut missing = Command::new(temp.path().join("missing"));
    let error = run_command(
        &mut missing,
        Provider::Claude,
        "private",
        &Cancel::default(),
        Duration::from_secs(1),
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(error, "Install `claude` and put it on PATH");
}

#[cfg(unix)]
fn waiting_provider(dir: &std::path::Path) -> Command {
    let mut shell = Command::new("/bin/sh");
    shell.current_dir(dir).args([
        "-c",
        r#"
        sleep 30 &
        printf '%s\n%s\n' "$$" "$!" > pids
        printf '%s\n' '{"type":"system","subtype":"init"}'
        wait
    "#,
    ]);
    shell
}

#[cfg(unix)]
fn assert_reaped_group(dir: &std::path::Path) {
    let pids = std::fs::read_to_string(dir.join("pids")).unwrap();
    let mut pids = pids.lines();
    let leader = pids.next().unwrap().parse::<libc::pid_t>().unwrap();
    let child = pids.next().unwrap();
    // SAFETY: signal 0 queries the real positive PID and sends no signal.
    assert_eq!(
        unsafe { libc::kill(leader, 0) },
        -1,
        "leader was not reaped"
    );
    let state = Command::new("ps")
        .args(["-o", "stat=", "-p", child])
        .output()
        .unwrap();
    let state = std::str::from_utf8(&state.stdout).unwrap().trim();
    assert!(
        state.is_empty() || state.starts_with('Z'),
        "descendant is still running: {state}"
    );
}

#[test]
#[cfg(unix)]
fn cancelling_review_provider_kills_the_group_and_reaps_the_leader() {
    let dir = tempfile::tempdir().unwrap();
    let token = Cancel::default();
    let answer = run_command(
        &mut waiting_provider(dir.path()),
        Provider::Claude,
        "question",
        &token,
        Duration::from_secs(5),
        &mut |_| token.cancel(),
    );
    assert!(answer.unwrap_err().contains("cancelled"));
    assert_reaped_group(dir.path());
}

#[test]
#[cfg(unix)]
fn stalled_review_provider_kills_the_group_and_reaps_the_leader() {
    let dir = tempfile::tempdir().unwrap();
    let answer = run_command(
        &mut waiting_provider(dir.path()),
        Provider::Claude,
        "question",
        &Cancel::default(),
        Duration::from_millis(250),
        &mut |_| {},
    );
    assert!(answer.unwrap_err().contains("stalled"));
    assert_reaped_group(dir.path());
}

#[test]
#[cfg(unix)]
fn companion_runtime_shutdown_waits_for_provider_reap() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = CompanionRuntime::default();
    let cancel = runtime.cancel_token();
    let path = dir.path().to_owned();
    let (started, ready) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        run_command(
            &mut waiting_provider(&path),
            Provider::Claude,
            "question",
            &cancel,
            Duration::from_secs(5),
            &mut |_| {
                let _ = started.send(());
            },
        )
    });
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    runtime.shutdown();
    assert_reaped_group(dir.path());
    assert!(worker.join().unwrap().unwrap_err().contains("cancelled"));
    assert!(runtime.cancel_token().is_cancelled());
    assert!(
        !Cancel::default().is_cancelled(),
        "detached jobs use separate tokens"
    );
}
