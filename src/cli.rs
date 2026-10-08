use std::collections::HashMap;

use serde::Serialize;

use crate::model::{occurrences, validate_occurrence_range, Date};
use crate::store::Store;

const USAGE: &str = "dot-calendar list [--date YYYY-MM-DD]\n\
dot-calendar occurrences --from YYYY-MM-DD --to YYYY-MM-DD\n\
dot-calendar add --date YYYY-MM-DD [--time HH:MM] --title TEXT [--notes TEXT] [--request-id ID] [--completed true|false] [--color #RRGGBB] [--recurrence daily|weekly|monthly|yearly] [--reminder-minutes N]\n\
dot-calendar update --id ID --date YYYY-MM-DD [--time HH:MM] --title TEXT [--notes TEXT] [--completed true|false] [--color #RRGGBB|none] [--recurrence daily|weekly|monthly|yearly|none] [--reminder-minutes N|none]\n\
dot-calendar details --id ID [--completed true|false] [--color #RRGGBB|none] [--recurrence daily|weekly|monthly|yearly|none] [--reminder-minutes N|none]\n\
dot-calendar delete --id ID\n\
dot-calendar backup-export [--file PATH]\n\
dot-calendar backup-restore --file PATH\n\
dot-calendar --mcp";

#[derive(Serialize)]
struct Help<'a> {
    usage: &'a str,
}

/// `args` starts with the subcommand (the executable name is already removed).
pub fn run(args: &[String]) -> i32 {
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h") {
        print_json(&Help { usage: USAGE });
        return 0;
    }
    let command = args[0].as_str();
    let permitted: &[&str] = match command {
        "list" => &["--date"],
        "occurrences" => &["--from", "--to"],
        "add" => &[
            "--date",
            "--time",
            "--title",
            "--notes",
            "--request-id",
            "--completed",
            "--color",
            "--recurrence",
            "--reminder-minutes",
        ],
        "update" => &[
            "--id",
            "--date",
            "--time",
            "--title",
            "--notes",
            "--completed",
            "--color",
            "--recurrence",
            "--reminder-minutes",
        ],
        "details" => &[
            "--id",
            "--completed",
            "--color",
            "--recurrence",
            "--reminder-minutes",
        ],
        "delete" => &["--id"],
        "backup-export" => &["--file"],
        "backup-restore" => &["--file"],
        _ => return usage_error("unknown command"),
    };
    let options = match parse_options(&args[1..], permitted) {
        Ok(options) => options,
        Err(error) => return usage_error(&error),
    };
    let require = |name: &str| {
        options
            .get(name)
            .copied()
            .ok_or_else(|| format!("missing {name}"))
    };
    let result = (|| {
        let store = Store::open()?;
        match command {
            "list" => print_json(&store.list_events(options.get("--date").copied())?),
            "occurrences" => {
                let from = Date::parse(require("--from")?)?;
                let to = Date::parse(require("--to")?)?;
                validate_occurrence_range(from, to)?;
                print_json(&occurrences(&store.list_events(None)?, from, to));
            }
            "add" => print_json(&store.create_event_with_details(
                require("--date")?,
                options.get("--time").copied(),
                require("--title")?,
                options.get("--notes").copied().unwrap_or(""),
                options.get("--request-id").copied(),
                optional_bool(&options, "--completed")?.unwrap_or(false),
                nullable_string(&options, "--color").flatten(),
                nullable_string(&options, "--recurrence").flatten(),
                nullable_u32(&options, "--reminder-minutes")?.flatten(),
            )?),
            "update" => print_json(&store.update_event_with_details(
                require("--id")?,
                require("--date")?,
                options.get("--time").copied(),
                require("--title")?,
                options.get("--notes").copied().unwrap_or(""),
                optional_bool(&options, "--completed")?,
                nullable_string(&options, "--color"),
                nullable_string(&options, "--recurrence"),
                nullable_u32(&options, "--reminder-minutes")?,
            )?),
            "details" => print_json(&store.patch_details(
                require("--id")?,
                optional_bool(&options, "--completed")?,
                nullable_string(&options, "--color"),
                nullable_string(&options, "--recurrence"),
                nullable_u32(&options, "--reminder-minutes")?,
            )?),
            "delete" => print_json(&store.delete_event(require("--id")?)?),
            "backup-export" => {
                let backup = store.export_json()?;
                if let Some(path) = options.get("--file") {
                    std::fs::write(path, backup.as_bytes())
                        .map_err(|error| format!("cannot write backup file: {error}"))?;
                    print_json(&serde_json::json!({"exported": path}));
                } else {
                    println!("{backup}");
                }
            }
            "backup-restore" => {
                let file = std::fs::read_to_string(require("--file")?)
                    .map_err(|error| format!("cannot read backup file: {error}"))?;
                store.restore_json(&file)?;
                print_json(&serde_json::json!({"restored": true}));
            }
            _ => unreachable!(),
        }
        Ok::<(), String>(())
    })();
    match result {
        Ok(()) => 0,
        Err(error) if error.starts_with("missing --") => usage_error(&error),
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn optional_bool(options: &HashMap<&str, &str>, key: &str) -> Result<Option<bool>, String> {
    match options.get(key).copied() {
        None => Ok(None),
        Some("true") => Ok(Some(true)),
        Some("false") => Ok(Some(false)),
        Some(_) => Err(format!("{key} must be true or false")),
    }
}

fn nullable_string<'a>(options: &HashMap<&str, &'a str>, key: &str) -> Option<Option<&'a str>> {
    options
        .get(key)
        .map(|value| if *value == "none" { None } else { Some(*value) })
}

fn nullable_u32(options: &HashMap<&str, &str>, key: &str) -> Result<Option<Option<u32>>, String> {
    match options.get(key).copied() {
        None => Ok(None),
        Some("none") => Ok(Some(None)),
        Some(value) => value
            .parse::<u32>()
            .map(Some)
            .map(Some)
            .map_err(|_| format!("{key} must be a nonnegative integer or none")),
    }
}

fn parse_options<'a>(
    args: &'a [String],
    permitted: &[&str],
) -> Result<HashMap<&'a str, &'a str>, String> {
    let mut options = HashMap::new();
    let (pairs, remainder) = args.as_chunks::<2>();
    for pair in pairs {
        let name = pair[0].as_str();
        if !permitted.contains(&name) {
            return Err(format!("unknown option {name}"));
        }
        if pair[1].starts_with("--") {
            return Err(format!("missing value for {name}"));
        }
        if options.insert(name, pair[1].as_str()).is_some() {
            return Err(format!("duplicate option {name}"));
        }
    }
    if let Some(name) = remainder.first() {
        return Err(format!("missing value for {name}"));
    }
    Ok(options)
}

fn print_json(value: &impl Serialize) {
    // Event and Vec<Event> serialization cannot fail with the built-in data types.
    println!(
        "{}",
        serde_json::to_string(value).expect("JSON serialization failed")
    );
}

fn usage_error(error: &str) -> i32 {
    eprintln!("{error}\n{USAGE}");
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_and_duplicate_options_before_opening_store() {
        assert_eq!(run(&["add".into(), "--date".into()]), 2);
        assert_eq!(
            run(&[
                "list".into(),
                "--date".into(),
                "2026-10-07".into(),
                "--date".into(),
                "2026-10-08".into()
            ]),
            2
        );
        assert_eq!(run(&["unknown".into()]), 2);
    }

    #[test]
    fn optional_detail_values_have_clear_and_preserve_states() {
        let mut options = HashMap::new();
        assert_eq!(nullable_string(&options, "--color"), None);
        options.insert("--color", "none");
        options.insert("--reminder-minutes", "15");
        options.insert("--completed", "true");
        assert_eq!(nullable_string(&options, "--color"), Some(None));
        assert_eq!(
            nullable_u32(&options, "--reminder-minutes").unwrap(),
            Some(Some(15))
        );
        assert_eq!(optional_bool(&options, "--completed").unwrap(), Some(true));
    }
}
