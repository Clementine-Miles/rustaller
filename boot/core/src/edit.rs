//! Format-preserving editing is linked only into Linux tooling.
use super::*;
use alloc::string::ToString;
use toml_edit::{Array, DocumentMut, Item, Table, value};

pub fn create(root: &str, rootfstype: &str, entries: &[Entry]) -> Result<String, Error> {
    let mut doc = DocumentMut::new();
    doc["schema_version"] = value(1);
    doc["boot"] = Item::Table(Table::new());
    doc["boot"]["default"] = value("current");
    doc["boot"]["timeout_seconds"] = value(3);
    doc["boot"]["serial"] = value(true);
    doc["boot"]["serial_baudrate"] = value(115200);
    doc["linux"] = Item::Table(Table::new());
    doc["linux"]["root"] = value(root);
    doc["linux"]["rootfstype"] = value(rootfstype);
    doc["linux"]["rootflags"] = value(if rootfstype == "btrfs" {
        "subvol=@"
    } else {
        ""
    });
    doc["linux"]["mode"] = value("rw");
    doc["linux"]["extra_cmdline"] = value("");
    publish(doc, entries)
}

pub fn replace_entries(text: &str, entries: &[Entry]) -> Result<String, Error> {
    Config::parse(text)?;
    let doc = text
        .parse::<DocumentMut>()
        .map_err(|_| Error("cannot edit boot.toml"))?;
    publish(doc, entries)
}

/// Reassert only the installer's UUID/filesystem identity; retain every other
/// user policy field, including comments and an explicit full cmdline override.
pub fn replace_entries_for_root(
    text: &str,
    entries: &[Entry],
    root: &str,
    rootfstype: &str,
) -> Result<String, Error> {
    Config::parse(text)?;
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|_| Error("cannot edit boot.toml"))?;
    string_value(&mut doc["linux"]["root"], root);
    string_value(&mut doc["linux"]["rootfstype"], rootfstype);
    publish(doc, entries)
}
fn string_value(item: &mut Item, text: &str) {
    let decor = item.as_value().map(|v| v.decor().clone());
    *item = value(text);
    if let Some(decor) = decor {
        *item.as_value_mut().expect("string value").decor_mut() = decor;
    }
}

fn publish(mut doc: DocumentMut, entries: &[Entry]) -> Result<String, Error> {
    validate_entries(entries)?;
    let mut section = Table::new();
    section.set_implicit(true);
    for entry in entries {
        let mut fields = doc
            .get("entries")
            .and_then(Item::as_table)
            .and_then(|table| table.get(&entry.name))
            .and_then(Item::as_table)
            .cloned()
            .unwrap_or_default();
        if let Some(v) = &entry.version {
            string_value(&mut fields["version"], v);
        } else {
            fields.remove("version");
        }
        string_value(&mut fields["kernel"], &entry.kernel);
        let decor = fields
            .get("initrds")
            .and_then(Item::as_value)
            .map(|v| v.decor().clone());
        fields["initrds"] = value(entry.initrds.iter().map(String::as_str).collect::<Array>());
        if let Some(decor) = decor {
            *fields["initrds"].as_value_mut().expect("array").decor_mut() = decor;
        }
        section[&entry.name] = Item::Table(fields);
    }
    doc["entries"] = Item::Table(section);
    let text = doc.to_string();
    Config::parse(&text)?;
    Ok(text)
}
