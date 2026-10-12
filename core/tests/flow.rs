//! End-to-end hook -> dispatcher flow against a temporary pipeline root,
//! with a recording speaker instead of a real TTS engine.

use std::fs::{self, File};
use std::path::Path;

use serde_json::json;
use tempfile::TempDir;
use voice_core::{dispatcher, hook, pipeline::Pipeline};

struct Env {
    _dir: TempDir,
    pipeline: Pipeline,
    transcript: std::path::PathBuf,
}

fn env() -> Env {
    let dir = TempDir::new().unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    File::create(&transcript).unwrap();
    let pipeline = Pipeline::new(dir.path());
    Env { _dir: dir, pipeline, transcript }
}

fn add_user_prompt(transcript: &Path, text: &str) {
    let line = json!({"type": "user", "message": {"role": "user", "content": text}});
    let mut existing = fs::read_to_string(transcript).unwrap();
    existing.push_str(&format!("{line}\n"));
    fs::write(transcript, existing).unwrap();
}

fn add_tool_result(transcript: &Path) {
    let line = json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}});
    let mut existing = fs::read_to_string(transcript).unwrap();
    existing.push_str(&format!("{line}\n"));
    fs::write(transcript, existing).unwrap();
}

fn add_assistant_text(transcript: &Path, texts: &[&str]) {
    let content: Vec<_> = texts.iter().map(|t| json!({"type": "text", "text": t})).collect();
    let line = json!({"type": "assistant", "message": {"content": content}});
    let mut existing = fs::read_to_string(transcript).unwrap();
    existing.push_str(&format!("{line}\n"));
    fs::write(transcript, existing).unwrap();
}

fn payload(env: &Env, session: &str, extra: serde_json::Value) -> String {
    let mut base = json!({
        "session_id": session,
        "cwd": "/home/me/project",
        "transcript_path": env.transcript,
    });
    base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    base.to_string()
}

fn post_tool_use(env: &Env, session: &str) {
    hook::run(&env.pipeline, &payload(env, session, json!({"tool_name": "Bash"})));
}

fn stop(env: &Env, session: &str, message: &str) {
    hook::run(&env.pipeline, &payload(env, session, json!({"last_assistant_message": message})));
}

fn drain(env: &Env) -> Vec<String> {
    let mut said = Vec::new();
    dispatcher::run(&env.pipeline, &mut |_, text| {
        said.push(text.to_string());
        true
    });
    said
}

fn pending_count(env: &Env) -> usize {
    fs::read_dir(env.pipeline.dispatch_dir())
        .map(|d| d.filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().ends_with(".not_dispatched")).count())
        .unwrap_or(0)
}

#[test]
fn first_sighting_baselines_without_speaking_history() {
    let env = env();
    add_assistant_text(&env.transcript, &["Old text."]);
    post_tool_use(&env, "s1");
    assert_eq!(pending_count(&env), 0);
    assert!(drain(&env).is_empty());
}

#[test]
fn new_blocks_are_queued_one_marker_per_line() {
    let env = env();
    post_tool_use(&env, "s1");
    add_assistant_text(&env.transcript, &["Primero.\n\nSegundo.", "Tercero."]);
    post_tool_use(&env, "s1");
    assert_eq!(drain(&env), vec!["Primero.", "Segundo.", "Tercero."]);
}

#[test]
fn stop_after_its_blocks_says_nothing_more() {
    let env = env();
    post_tool_use(&env, "s1");
    add_assistant_text(&env.transcript, &["Primero.\n\nSegundo."]);
    stop(&env, "s1", "Primero.\n\nSegundo.");
    assert_eq!(drain(&env), vec!["Primero.", "Segundo."]);
}

#[test]
fn stop_says_the_block_missing_from_the_transcript_and_it_is_not_repeated() {
    let env = env();
    post_tool_use(&env, "s1");
    add_assistant_text(&env.transcript, &["Primero."]);
    stop(&env, "s1", "Primero.\n\nEste cierre todavía no llegó al transcript.");
    assert_eq!(drain(&env), vec!["Primero.", "Este cierre todavía no llegó al transcript."]);

    // Next turn: the late block finally shows up in the transcript.
    add_assistant_text(&env.transcript, &["Este cierre todavía no llegó al transcript."]);
    post_tool_use(&env, "s1");
    assert!(drain(&env).is_empty());
}

#[test]
fn a_phrase_said_days_ago_does_not_swallow_a_new_paragraph() {
    let env = env();
    post_tool_use(&env, "old");
    add_assistant_text(&env.transcript, &["¿Cuál preferís?"]);
    stop(&env, "old", "¿Cuál preferís?");
    drain(&env);

    post_tool_use(&env, "s1");
    add_assistant_text(&env.transcript, &["Siguiente paso: T3 o T4. ¿Cuál preferís?"]);
    stop(&env, "s1", "Siguiente paso: T3 o T4. ¿Cuál preferís?");
    // Both sessions spoke within 15 minutes, hence the session tag.
    assert_eq!(drain(&env), vec!["Sesión project: Siguiente paso: T3 o T4. ¿Cuál preferís?"]);
}

#[test]
fn ask_user_question_reads_the_question_and_options() {
    let env = env();
    post_tool_use(&env, "s1");
    let extra = json!({
        "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [
            {"question": "¿Rust o Python?", "options": [{"label": "Rust"}, {"label": "Python"}]},
            {"question": "¿Seguro?", "options": []}
        ]}
    });
    hook::run(&env.pipeline, &payload(&env, "s1", extra));
    assert_eq!(
        drain(&env),
        vec!["Pregunta: ¿Rust o Python?. Opciones: Rust, Python.\n\nPregunta: ¿Seguro?."]
    );
}

#[test]
fn markers_created_while_disabled_are_never_spoken() {
    let env = env();
    post_tool_use(&env, "s1");
    env.pipeline.config.set_disabled(true).unwrap();
    add_assistant_text(&env.transcript, &["Silencio."]);
    post_tool_use(&env, "s1");
    env.pipeline.config.set_disabled(false).unwrap();
    assert!(drain(&env).is_empty());
}

#[test]
fn disabling_mid_queue_finalizes_without_speaking() {
    let env = env();
    post_tool_use(&env, "s1");
    add_assistant_text(&env.transcript, &["Uno.\nDos."]);
    post_tool_use(&env, "s1");
    let mut said = Vec::new();
    dispatcher::run(&env.pipeline, &mut |p, text| {
        said.push(text.to_string());
        p.config.set_disabled(true).unwrap();
        true
    });
    assert_eq!(said, vec!["Uno."]);
    assert_eq!(pending_count(&env), 0);
}

#[test]
fn concurrent_sessions_are_tagged_with_their_folder() {
    let env = env();
    let other = env.transcript.with_file_name("other.jsonl");
    File::create(&other).unwrap();
    let s2_hook = |env: &Env| {
        let extra = json!({"tool_name": "Bash", "transcript_path": other, "cwd": "/home/me/other"});
        hook::run(&env.pipeline, &payload(env, "s2", extra));
    };
    post_tool_use(&env, "s1");
    s2_hook(&env);
    // A session counts as active once it has spoken (tag_if_multi_session).
    add_assistant_text(&other, &["Otra."]);
    s2_hook(&env);
    add_assistant_text(&env.transcript, &["Hola."]);
    post_tool_use(&env, "s1");
    assert_eq!(drain(&env), vec!["Otra.", "Sesión project: Hola."]);
}

#[test]
fn a_second_dispatcher_exits_while_one_holds_the_lock() {
    let env = env();
    post_tool_use(&env, "s1");
    add_assistant_text(&env.transcript, &["Hola."]);
    post_tool_use(&env, "s1");

    let lock = File::create(env.pipeline.tmp().join("dispatcher.lock")).unwrap();
    lock.lock().unwrap();
    assert!(drain(&env).is_empty());
    assert_eq!(pending_count(&env), 1);
    drop(lock);
    assert_eq!(drain(&env), vec!["Hola."]);
}

#[test]
fn short_lines_of_a_late_final_message_are_not_repeated_next_turn() {
    let env = env();
    add_user_prompt(&env.transcript, "Hacé la tabla.");
    post_tool_use(&env, "s1");
    add_assistant_text(&env.transcript, &["Intro."]);
    // The final message is not in the transcript yet when Stop fires.
    let last = "**Verificado**\nmacOS.\n4,4 MB.\n¿Lo pusheo?";
    stop(&env, "s1", &format!("Intro.\n\n{last}"));
    assert_eq!(drain(&env), vec!["Intro.", "**Verificado**\nmacOS.\n4,4 MB.\n¿Lo pusheo?"]);

    // Next turn: the late message lands before the new prompt, then new work.
    add_assistant_text(&env.transcript, &[last]);
    add_user_prompt(&env.transcript, "Dale.");
    add_assistant_text(&env.transcript, &["Listo."]);
    add_tool_result(&env.transcript);
    post_tool_use(&env, "s1");
    assert_eq!(drain(&env), vec!["Listo."]);
}

#[test]
fn tool_results_do_not_end_a_turn() {
    let env = env();
    add_user_prompt(&env.transcript, "Arrancá.");
    post_tool_use(&env, "s1");
    add_assistant_text(&env.transcript, &["Primero."]);
    add_tool_result(&env.transcript);
    add_assistant_text(&env.transcript, &["Segundo."]);
    post_tool_use(&env, "s1");
    assert_eq!(drain(&env), vec!["Primero.", "Segundo."]);
}
