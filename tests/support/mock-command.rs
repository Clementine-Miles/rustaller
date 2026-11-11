use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::Path;

fn main() {
    let mut input = Vec::new();
    if io::stdin().read_to_end(&mut input).is_err() {
        std::process::exit(90);
    }

    let mut arguments = std::env::args_os();
    let program = arguments
        .next()
        .as_deref()
        .map(Path::new)
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let args: Vec<String> = arguments
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let default_stdout = format!(
        "out:{}:{}\n",
        args.first().map_or("", String::as_str),
        String::from_utf8_lossy(&input)
    );
    let stdout = if program == "mount" && args.is_empty() {
        std::env::var("MOCK_MOUNT_OUTPUT").unwrap_or(default_stdout)
    } else {
        default_stdout
    };
    if io::stdout().write_all(stdout.as_bytes()).is_err() {
        std::process::exit(91);
    }
    let _ = writeln!(io::stderr(), "diagnostic");
    let status = std::env::var("MOCK_EXIT_CODE")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);

    if let Some(trace_path) = std::env::var_os("MOCK_TRACE_PATH") {
        if append_trace(&trace_path, &program, &args, &input, status).is_err() {
            std::process::exit(92);
        }
    }
    std::process::exit(status);
}

fn append_trace(
    path: &std::ffi::OsStr,
    program: &str,
    args: &[String],
    stdin: &[u8],
    status: i32,
) -> io::Result<()> {
    let phase = std::env::var("MOCK_TRACE_PHASE").unwrap_or_default();
    let target = std::env::var("MOCK_TRACE_TARGET").unwrap_or_default();
    let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
    let mut env_delta = BTreeMap::new();
    let mut selected: Vec<String> = std::env::var("MOCK_TRACE_ENV_KEYS")
        .unwrap_or_default()
        .split(',')
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .collect();
    selected.sort();
    selected.dedup();
    for key in selected {
        if let Some(value) = std::env::var_os(&key) {
            env_delta.insert(key, value.to_string_lossy().into_owned());
        }
    }

    let argv = args
        .iter()
        .map(|argument| json_string(argument))
        .collect::<Vec<_>>()
        .join(",");
    let env_delta = env_delta
        .iter()
        .map(|(key, value)| format!("{}:{}", json_string(key), json_string(value)))
        .collect::<Vec<_>>()
        .join(",");
    let record = format!(
        "{{\"phase\":{},\"target\":{},\"program\":{},\"argv\":[{}],\"cwd\":{},\"env_delta\":{{{}}},\"stdin_fixture\":{},\"expected_status\":{status}}}\n",
        json_string(&phase),
        json_string(&target),
        json_string(program),
        argv,
        json_string(&cwd),
        env_delta,
        json_string(&String::from_utf8_lossy(stdin)),
    );
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(record.as_bytes())
}

fn json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}
