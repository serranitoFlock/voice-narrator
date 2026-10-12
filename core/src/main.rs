//! voice-speak: cross-platform replacement for the voice-pipeline scripts.

use std::io::Read;
use std::process::{Command, ExitCode, Stdio};

use voice_core::config::Engine;
use voice_core::pipeline::Pipeline;
use voice_core::{dispatcher, hook, tts};

const USAGE: &str = "\
Usage: voice-speak [command]

  hook              Claude Code hook (default): reads the payload on stdin
  dispatch          Drain the narration queue (started by the hook)
  say [text]        Speak text now (from arguments or stdin)
  clean [text]      Print the text as the TTS engine would receive it
  on | off | status Turn narration on/off, or show its state
  engine [kokoro|piper|status]
                    Switch or show the TTS engine
";

fn main() -> ExitCode {
    let pipeline = Pipeline::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("hook");
    let rest = &args[args.len().min(1)..];

    match command {
        "hook" => {
            // A hook must never fail or block Claude Code.
            let mut input = String::new();
            if std::io::stdin().read_to_string(&mut input).is_ok() {
                hook::run(&pipeline, &input);
            }
            spawn_dispatcher(&pipeline);
            ExitCode::SUCCESS
        }
        "dispatch" => {
            dispatcher::run(&pipeline, &mut |p, text| tts::synthesize_and_play(p, text));
            ExitCode::SUCCESS
        }
        "say" => match text_arg(rest) {
            Some(text) if tts::synthesize_and_play(&pipeline, &text) => ExitCode::SUCCESS,
            Some(_) => ExitCode::FAILURE,
            None => usage("Uso: voice-speak say <texto> (o por stdin)"),
        },
        "clean" => match text_arg(rest) {
            Some(text) => {
                println!("{}", tts::prepare(&pipeline, &text).join("\n"));
                ExitCode::SUCCESS
            }
            None => usage("Uso: voice-speak clean <texto> (o por stdin)"),
        },
        "on" | "off" => {
            let off = command == "off";
            match pipeline.config.set_disabled(off) {
                Ok(()) => {
                    println!("Narración de voz {}.", if off { "DESACTIVADA" } else { "ACTIVADA" });
                    ExitCode::SUCCESS
                }
                Err(e) => usage(&format!("No se pudo cambiar el estado: {e}")),
            }
        }
        "status" => {
            let state = if pipeline.config.is_disabled() { "desactivada" } else { "activada" };
            println!("Estado: {state}");
            ExitCode::SUCCESS
        }
        "engine" => match rest.first().map(String::as_str).unwrap_or("status") {
            "status" => {
                println!("Motor TTS actual: {}", pipeline.config.engine().as_str());
                ExitCode::SUCCESS
            }
            name @ ("kokoro" | "piper") => {
                let engine = if name == "piper" { Engine::Piper } else { Engine::Kokoro };
                match pipeline.config.set_engine(engine) {
                    Ok(()) => {
                        println!("Motor TTS: {name}");
                        ExitCode::SUCCESS
                    }
                    Err(e) => usage(&format!("No se pudo cambiar el motor: {e}")),
                }
            }
            _ => usage("Uso: voice-speak engine kokoro|piper|status"),
        },
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => usage(USAGE),
    }
}

fn usage(message: &str) -> ExitCode {
    eprint!("{message}");
    if !message.ends_with('\n') {
        eprintln!();
    }
    ExitCode::FAILURE
}

/// Text from the remaining arguments, or stdin when there are none.
fn text_arg(rest: &[String]) -> Option<String> {
    let text = if rest.is_empty() {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).ok()?;
        input
    } else {
        rest.join(" ")
    };
    (!text.trim().is_empty()).then_some(text)
}

/// Starts `voice-speak dispatch` detached, so the hook returns at once
/// (hook timeouts are 10-15s; a backlog can take minutes). If a dispatcher
/// is already running, the new one sees the lock and exits immediately.
fn spawn_dispatcher(pipeline: &Pipeline) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = Command::new(exe);
    command.arg("dispatch").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    if let Err(e) = command.spawn() {
        pipeline.log(&format!("could not start the dispatcher: {e}"));
    }
}
