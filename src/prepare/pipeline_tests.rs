use std::cell::RefCell;
use std::collections::VecDeque;

use super::*;
use crate::model::{FilePatch, FileStatus};
use crate::review_agent::Provider;

fn draft() -> ReviewGuide {
    serde_json::from_value(json!({
        "version": 2,
        "diff": "current",
        "summary": "Add the greeting",
        "stories": [{"title": "Greet a caller", "stops": [
            {"id": "edited", "claim": "A greeting is returned", "path": "lib.rs", "start_line": 50, "end_line": 51},
            {"id": "discarded", "claim": "Ghost code works", "path": "ghost.rs", "start_line": 9, "end_line": 9}
        ]}],
        "estimate_minutes": 2
    })).unwrap()
}

#[test]
fn preparation_extracts_the_expected_json_from_messy_output() {
    let output = format!(
        "A thought about {{source}}\n{{\"usage\":3}}\n```json\n{}\n```\nEnd.",
        serde_json::to_string(&draft()).unwrap()
    );
    assert_eq!(extract::<ReviewGuide>(&output, "guide").unwrap(), draft());
    assert!(
        extract::<ReviewGuide>("No JSON here", "guide")
            .unwrap_err()
            .to_string()
            .contains("retry")
    );
}

#[test]
fn preparation_fake_runner_repairs_once_and_records_dropped_stops() {
    let files = crate::vcs::diff_parser::parse_file_patches(vec![FilePatch::new(
        None,
        Some("lib.rs".into()),
        FileStatus::Added,
        "@@ -0,0 +1,2 @@\n+fn greet() -> &'static str { \"hi\" }\n+\n",
    )])
    .unwrap();
    let context_data = json!({ "description": "Add greeting" });
    let context = ReviewContext {
        files: &files,
        diff: "current",
        mechanical: &mechanical::classify(&files),
        outline: outline::outline(&files, "current"),
        context: &context_data,
        rules: &Rules::default(),
        evidence: "",
        source_note: None,
    };
    let mut previous = draft();
    previous.diff = "prior".into();
    let answers = RefCell::new(VecDeque::from([
        json!({"guide": draft(), "audit": {"stops": [
            {"id": "edited", "audit": {"result": "holds"}, "untested": true},
            {"id": "discarded", "audit": {"result": "doubtful", "note": "No source."}}
        ]}}),
        json!({"anchors": [{"id": "edited", "path": "lib.rs", "side": "new", "start_line": 1, "end_line": 1}]}),
        json!({"stops": [{"id": "edited", "audit": {"result": "holds"}, "untested": true}]}),
    ]));
    let fake = |request: &AgentRequest, _: &Cancel, on_event: &mut dyn FnMut(AgentEvent<'_>)| {
        match answers.borrow().len() {
            3 => {
                assert!(request.prompt.contains("previous_guide"));
                assert!(request.prompt.contains("\"diff\":\"prior\""));
            }
            2 => {
                assert!(request.prompt.contains("resolver_error"));
                assert!(request.prompt.contains("lib.rs (new side, lines 50-51)"));
                assert!(request.prompt.contains("ghost.rs (new side, lines 9-9)"));
            }
            1 => assert!(request.prompt.contains("\"only_stop_ids\":[\"edited\"]")),
            _ => panic!("extra provider call"),
        }
        on_event(AgentEvent::Activity("Reading source"));
        on_event(AgentEvent::Text("output"));
        Ok(answers
            .borrow_mut()
            .pop_front()
            .expect("only one repair round")
            .to_string())
    };
    let settings = Settings {
        provider: Provider::Claude,
        ask_model: None,
        reviewer_model: None,
        prepare_on_open: false,
    };
    let mut state = PrepState::started("Starting", Some("current".into()), None);
    let mut model = Model {
        settings: &settings,
        dir: Path::new("."),
        cancel: &Cancel::default(),
        runner: &fake,
    };
    let mut provider_heartbeats = 0;
    let mut stages = Vec::new();
    let (mut guide, dropped) = context
        .write_audit_and_validate(None, Some(&previous), &mut |stage, request| {
            stages.push(stage.to_owned());
            state.step = stage.into();
            match request {
                Some(request) => model.call(request, &mut state, &mut |event_state| {
                    assert!(event_state.heartbeat_at >= event_state.started_at);
                    provider_heartbeats += 1;
                    Ok(())
                }),
                None => Ok(String::new()),
            }
        })
        .unwrap();
    let unaudited = unaudited_count(&guide);
    guide.provenance.limits = preparation_limits(None, &[], false, dropped, unaudited);
    assert!(answers.borrow().is_empty());
    assert_eq!(
        stages,
        [
            "Writing and auditing the review guide",
            "Completing coverage before audit",
            "Validating stops",
            "Repairing stop anchors",
            "Refreshing coverage after audit",
            "Auditing unchecked stops"
        ]
    );
    assert_eq!(provider_heartbeats, 6);
    assert_eq!(guide.stop_count(), 1);
    let stop = guide.stop("edited").unwrap();
    assert_eq!(stop.start_line, 1);
    assert_eq!(stop.audit, Audit::Holds);
    assert_eq!(unaudited, 0);
    assert!(guide.provenance.audited);
    assert!(
        guide
            .provenance
            .limits
            .unwrap()
            .contains("Dropped 1 unresolvable stop")
    );
}

#[test]
fn preparation_audit_replaces_test_evidence_and_places_unexplained_risks_in_story() {
    let mut guide = draft();
    let mut missed = guide.stories[0].stops[0].clone();
    missed.id = "unexplained".into();
    let response = serde_json::from_value(json!({
        "stops": [{"id": "edited", "audit": {"result": "holds"}, "test": {"path": "tests.rs", "name": "greeting"}, "untested": false}],
        "missed": [{"story_stop_id": "edited", "stop": missed}]
    })).unwrap();
    merge_audit(&mut guide, response, None);
    let edited = guide.stop("edited").unwrap();
    assert_eq!(edited.audit, Audit::Holds);
    assert_eq!(edited.test.as_ref().unwrap().name, "greeting");
    assert!(!edited.untested);
    assert_eq!(guide.stop("discarded").unwrap().audit, Audit::Unchecked);
    assert_eq!(unaudited_count(&guide), 2);
    assert_eq!(guide.stories.len(), 1);
    assert_eq!(
        guide.stories[0].stops[2].risk.as_ref().unwrap().kind,
        RiskKind::Unexplained
    );
}

#[test]
fn preparation_fake_runner_fails_with_the_stage_and_provider_reason() {
    let settings = Settings {
        provider: Provider::Claude,
        ask_model: None,
        reviewer_model: None,
        prepare_on_open: false,
    };
    let fake = |_: &AgentRequest, _: &Cancel, _: &mut dyn FnMut(AgentEvent<'_>)| {
        Err("claude is not installed".into())
    };
    let cancel = Cancel::default();
    let mut model = Model {
        settings: &settings,
        dir: Path::new("."),
        cancel: &cancel,
        runner: &fake,
    };
    let mut state = PrepState::started("Auditing every stop", None, None);
    let error = model
        .call("prompt".into(), &mut state, &mut |_| Ok(()))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Auditing every stop: claude is not installed"),
        "{error}"
    );
}

#[test]
fn preparation_audits_mechanical_fallback_and_replaces_false_prose() {
    let files = crate::vcs::diff_parser::parse_file_patches(vec![FilePatch::new(
        Some("tests/checkout.rs".into()),
        Some("tests/checkout.rs".into()),
        FileStatus::Modified,
        "@@ -1,3 +10,3 @@ fn checkout() {\n-assert_eq!(cart.count(), 1);\n+assert_eq!(cart.count(), 2);\n context\n-assert_eq!(stock, 1);\n+assert_eq!(stock, -1);\n",
    )]).unwrap();
    let mut guide = draft();
    guide.stories[0].summary = "Checkout is all-or-nothing.".into();
    guide.stories[0].stops.truncate(1);
    let stop = &mut guide.stories[0].stops[0];
    stop.path = "tests/checkout.rs".into();
    stop.start_line = 10;
    stop.end_line = 10;
    stop.explanation =
        "Checkout receives the submitted cart. Every cart line reserves stock atomically.".into();
    stop.followups = vec![Followup {
        question: "Are repeated SKUs safe?".into(),
        answer: "Yes; the order always goes through atomically.".into(),
    }];
    stop.test = Some(TestRef {
        path: "tests/checkout.rs".into(),
        name: "single_line".into(),
        line: None,
    });
    guide.estimate_minutes = None;
    let context = ReviewContext {
        files: &files,
        diff: "current",
        mechanical: &mechanical::classify(&files),
        outline: outline::outline(&files, "current"),
        context: &Value::Null,
        rules: &Rules::default(),
        evidence: "",
        source_note: None,
    };
    let mut audit_calls = 0;
    let (guide, dropped) = context.write_audit_and_validate(Some(guide), None, &mut |stage, prompt| {
        let Some(prompt) = prompt else { return Ok(String::new()); };
        audit_calls += 1;
        assert_eq!(stage, "Auditing every stop");
        let (_, data) = prompt.split_once("Additional input: ").unwrap();
        let payload: Value = serde_json::from_str(data.lines().next().unwrap()).unwrap();
        let draft: ReviewGuide = serde_json::from_value(payload["guide_to_audit"].clone()).unwrap();
        assert_eq!(draft.stop_count(), 2, "attach the old-side risk to the existing new-side stop");
        assert_eq!(draft.stop("edited").unwrap().risk.as_ref().unwrap().kind, RiskKind::TestChanged);
        let fallback = &draft.stories[1].stops[0];
        assert_eq!(fallback.start_line, 3);
        assert_eq!(fallback.symbol.as_deref(), Some("fn checkout() {"));
        assert!(fallback.untested);
        assert_eq!(fallback.audit, Audit::Unchecked);
        Ok(json!({"stops": [
            {
                "id": "edited",
                "audit": {"result":"holds"},
                "counterexample": "Two cart lines of 2 against 3 in stock each pass and leave -1. I could not confirm that repeated items are covered. The existing test checks only one cart line.",
                "test": {"path":"tests/checkout.rs","name":"single_line"},
                "untested": true,
                "explanation": "The code supports the claim. The test only exercises one item. I could not confirm the flag.",
                "explanation_edits": [
                    {
                        "sentence": "Every cart line reserves stock atomically.",
                        "replacement": "The checkout loop validates each line without reserving stock."
                    },
                    {
                        "sentence": "Checkout receives the submitted cart.",
                        "replacement": "The code supports the claim. I could not confirm the flag."
                    },
                    {
                        "sentence": "Checkout receives",
                        "replacement": "The auditor could not confirm this."
                    }
                ],
                "followups": [],
            },
            {"id": fallback.id, "audit": {"result":"holds"}, "untested":true}
        ]}).to_string())
    }).unwrap();
    assert_eq!(audit_calls, 1);
    assert_eq!(dropped, 0);
    assert_eq!(unaudited_count(&guide), 0);
    assert_eq!(guide.estimate_minutes, Some(4));
    assert!(guide.summary.contains("Stops cover 1 of 1 changed hunks"));
    let stop = guide.stop("edited").unwrap();
    assert_eq!(
        stop.explanation,
        "Checkout receives the submitted cart. The checkout loop validates each line without reserving stock."
    );
    assert!(stop.followups.is_empty());
    assert_eq!(
        stop.audit,
        Audit::Doubtful {
            note: "Two cart lines of 2 against 3 in stock each pass and leave -1. I could not confirm that repeated items are covered.".into(),
        }
    );
    assert!(stop.untested);
    assert!(stop.test.is_none());
    assert!(stop.needs_explicit_verdict());
    assert_eq!(guide.stories[1].stops[0].audit, Audit::Holds);
}

#[test]
fn preparation_caps_writer_orientation_and_followups_at_sentence_boundaries() {
    let files = changed_hunks(2);
    let context = ReviewContext {
        files: &files,
        diff: "current",
        mechanical: &mechanical::classify(&files),
        outline: outline::outline(&files, "current"),
        context: &Value::Null,
        rules: &Rules::default(),
        evidence: "",
        source_note: None,
    };
    let mut guide = draft();
    let short = concat!(
        "The caller receives a request and looks up the region in its configuration before it passes the request to checkout. ",
        "The new validation step checks that the region still has delivery slots and returns a message before any charge is attempted."
    );
    let overflow = "Checkout can then use the returned message to offer pickup, retry delivery, or ask the original caller to try a different address.";
    let first = &mut guide.stories[0].stops[0];
    first.start_line = 10;
    first.end_line = 10;
    first.explanation = format!("{short} {overflow}");
    let second = &mut guide.stories[0].stops[1];
    second.path = "lib.rs".into();
    second.start_line = 20;
    second.end_line = 20;
    second.explanation = "The caller checks again. Delivery is possible! Is pickup allowed? The store makes that choice. The caller shows an error.".into();
    let (guide, _) = context
        .write_audit_and_validate(None, None, &mut |stage, prompt| {
            if let Some(prompt) = prompt {
                assert_eq!(stage, "Writing and auditing the review guide");
                assert!(prompt.contains("at most 60 words"));
                assert!(prompt.contains("at most three sentences"));
                Ok(json!({
                    "guide": guide,
                    "audit": {"stops": [
                        {"id": "edited", "audit": {"result": "holds"}, "followups": [{
                            "question": "What happens next?",
                            "answer": "The caller gets a message. No charge is attempted! Could the user try again? That depends on the address."
                        }]},
                        {"id": "discarded", "audit": {"result": "holds"}}
                    ]}
                }).to_string())
            } else {
                Ok(String::new())
            }
        })
        .unwrap();
    let first = guide.stop("edited").unwrap();
    assert_eq!(first.explanation, short);
    assert_eq!(
        first.followups[0].answer,
        "The caller gets a message. No charge is attempted! Could the user try again?"
    );
    assert_eq!(
        guide.stop("discarded").unwrap().explanation,
        "The caller checks again. Delivery is possible! Is pickup allowed? The store makes that choice."
    );
}

fn changed_hunks(count: u32) -> Vec<DiffFile> {
    let mut patch = String::new();
    for index in 1..=count {
        let line = index * 10;
        writeln!(
            patch,
            "@@ -{line} +{line} @@\n-before_{index}();\n+after_{index}();"
        )
        .unwrap();
    }
    crate::vcs::diff_parser::parse_file_patches(vec![FilePatch::new(
        Some("lib.rs".into()),
        Some("lib.rs".into()),
        FileStatus::Modified,
        &patch,
    )])
    .unwrap()
}

#[test]
fn preparation_audits_missed_stops_and_limits_retries_for_omissions() {
    let files = changed_hunks(2);
    let context = ReviewContext {
        files: &files,
        diff: "current",
        mechanical: &mechanical::classify(&files),
        outline: outline::outline(&files, "current"),
        context: &json!({}),
        rules: &Rules::default(),
        evidence: "",
        source_note: None,
    };
    let mut guide = draft();
    guide.stories[0].stops.truncate(1);
    guide.stories[0].stops[0].start_line = 10;
    guide.stories[0].stops[0].end_line = 10;
    for resolves_on in [1, 2, 3] {
        let mut supplemental_calls = 0;
        let (guide, dropped) = context
            .write_audit_and_validate(Some(guide.clone()), None, &mut |stage, prompt| {
                let Some(prompt) = prompt else {
                    return Ok(String::new());
                };
                let response = if stage == "Auditing every stop" {
                    json!({
                        "stops": [{"id": "edited", "audit": {"result": "holds"}}],
                        "missed": [{"story_stop_id": "edited", "stop": {
                            "id": "missed", "claim": "Another caller now runs",
                            "path": "lib.rs", "start_line": 20, "end_line": 20
                        }}]
                    })
                } else {
                    assert_eq!(stage, "Auditing unchecked stops");
                    assert!(prompt.contains("\"only_stop_ids\":[\"missed\"]"));
                    supplemental_calls += 1;
                    if supplemental_calls == resolves_on {
                        json!({"stops": [{"id": "missed", "audit": {"result": "holds"}}]})
                    } else {
                        json!({"stops": []})
                    }
                };
                Ok(response.to_string())
            })
            .unwrap();
        let audited = resolves_on <= 2;
        assert_eq!(supplemental_calls, resolves_on.min(2));
        assert_eq!(dropped, 0);
        assert_eq!(guide.provenance.audited, audited);
        assert_eq!(guide.stop("edited").unwrap().audit, Audit::Holds);
        let missed = guide.stop("missed").unwrap();
        assert_eq!(missed.risk.as_ref().unwrap().kind, RiskKind::Unexplained);
        assert_eq!(
            missed.audit,
            if audited {
                Audit::Holds
            } else {
                Audit::Unchecked
            }
        );
        let limits = preparation_limits(None, &[], false, dropped, unaudited_count(&guide));
        assert_eq!(
            limits.as_deref(),
            (!audited).then_some("1 stop(s) were not audited")
        );
    }
}

#[test]
fn preparation_caps_model_story_stops_without_losing_risks_or_hunks() {
    let files = changed_hunks(19);
    let context = ReviewContext {
        files: &files,
        diff: "current",
        mechanical: &mechanical::classify(&files),
        outline: outline::outline(&files, "current"),
        context: &json!({}),
        rules: &Rules::default(),
        evidence: "",
        source_note: None,
    };
    let mut guide = draft();
    for (title, range) in [
        ("Primary behavior", 1..=10),
        ("Secondary behavior", 11..=19),
    ] {
        let mut story = Story {
            title: title.into(),
            summary: String::new(),
            stops: Vec::new(),
        };
        for index in range {
            let mut stop = guide.stories[0].stops[0].clone();
            stop.id = format!("stop-{index}");
            stop.claim = format!("Behavior {index} changes");
            stop.start_line = index * 10;
            stop.end_line = stop.start_line;
            if index == 1 || (16..=18).contains(&index) {
                stop.risk = Some(Risk {
                    kind: RiskKind::Unexplained,
                    reason: "Not in the description.".into(),
                });
            }
            story.stops.push(stop);
        }
        guide.stories.push(story);
    }
    guide.stories.remove(0);
    let audit = json!({
        "stops": (1..=19).map(|index| {
            if index == 19 {
                json!({"id": "stop-19", "audit": {"result": "doubtful", "note": "The caller can bypass it."}})
            } else {
                json!({
                    "id": format!("stop-{index}"),
                    "audit": {"result": "holds"},
                    "test": {"path": "tests.rs", "name": format!("covers_{index}")},
                    "untested": false
                })
            }
        }).collect::<Vec<_>>()
    });
    let (guide, dropped) = context
        .write_audit_and_validate(None, None, &mut |stage, prompt| {
            if prompt.is_none() {
                return Ok(String::new());
            }
            match stage {
                "Writing the review guide" => Ok(serde_json::to_string(&guide).unwrap()),
                "Auditing every stop" => Ok(audit.to_string()),
                _ => panic!("unexpected provider stage: {stage}"),
            }
        })
        .unwrap();
    let kept: Vec<_> = guide.stops().map(|(_, stop)| stop.id.clone()).collect();
    assert_eq!(
        kept,
        (1..=13)
            .chain(16..=19)
            .map(|i| format!("stop-{i}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(dropped, 0);
    assert!(guide.provenance.audited);
    assert!(guide.summary.contains("17 stops, over the 16-stop target"));
    assert_eq!(guide.estimate_minutes, Some(22));
    assert_eq!(guide.supporting.len(), 1);
    let group = &guide.supporting[0];
    assert_eq!(group.label, "More on Secondary behavior");
    assert_eq!(group.basis, Basis::Claimed);
    assert_eq!(group.note, "• Behavior 14 changes\n• Behavior 15 changes");
    assert_eq!(
        group
            .hunks
            .iter()
            .map(|hunk| hunk.new_start)
            .collect::<Vec<_>>(),
        [140, 150]
    );
    assert!(mechanical::uncovered_hunks(&guide, &files).is_empty());
}
