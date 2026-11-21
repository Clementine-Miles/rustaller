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

fn publish(mut doc: DocumentMut, entries: &[Entry]) -> Result<String, Error> {
    validate_entries(entries)?;
    let mut section = Table::new();
    section.set_implicit(true);
    for entry in entries {
        let mut fields = Table::new();
        if let Some(v) = &entry.version {
            fields["version"] = value(v);
        }
        fields["kernel"] = value(&entry.kernel);
        fields["initrds"] = value(entry.initrds.iter().map(String::as_str).collect::<Array>());
        section[&entry.name] = Item::Table(fields);
    }
    doc["entries"] = Item::Table(section);
    let text = doc.to_string();
    Config::parse(&text)?;
    Ok(text)
}
