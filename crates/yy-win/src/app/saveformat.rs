//! Save-dialog defaults. Inspect a bounded prefix, leaving the document untouched.
use super::*;
use std::path::Path;

const SAMPLE_BYTES: u64 = 64 << 10;

pub(super) struct Suggestion {
    pub name: String,
    pub extension: String,
}

impl App {
    pub(super) fn save_suggestion(&self, current: Option<&Path>) -> Suggestion {
        let snap = self.doc.snapshot();
        let head = snap.read(0..snap.len().min(SAMPLE_BYTES));
        let mode = if self.outline_mode {
            Some("json")
        } else if self.markdown_mode {
            Some("md")
        } else if self.hex.is_some() || self.code.is_some() {
            Some("bin")
        } else {
            self.csv
                .as_ref()
                .and_then(|csv| match csv.view.dialect.delimiter() {
                    b"," => Some("csv"),
                    b"\t" => Some("tsv"),
                    _ => None,
                })
        };
        let syntax = self
            .syntax
            .as_ref()
            .filter(|s| s.manual)
            .map(|s| s.view.syntax().id.as_str());
        suggest(
            current,
            self.outline_mode,
            mode,
            syntax,
            &head,
            &self.syntaxes,
        )
    }
}

fn suggest(
    current: Option<&Path>,
    outline: bool,
    mode: Option<&str>,
    syntax: Option<&str>,
    head: &[u8],
    registry: &yy_core::syntax::Registry,
) -> Suggestion {
    let existing = current.and_then(Path::extension).filter(|e| !e.is_empty());
    if !outline
        && existing.is_none()
        && let Some(name) = current.and_then(Path::file_name)
        && (name.to_string_lossy().starts_with('.')
            || matches!(
                registry.detect(current, head).as_deref(),
                Some("makefile" | "dockerfile" | "gitignore")
            ))
    {
        return Suggestion {
            name: name.to_string_lossy().into_owned(),
            extension: String::new(),
        };
    }
    let extension = if outline {
        "json".to_owned()
    } else if let Some(ext) = existing {
        ext.to_string_lossy().into_owned()
    } else {
        mode.map(str::to_owned)
            .or_else(|| syntax.and_then(extension_for_syntax).map(str::to_owned))
            .unwrap_or_else(|| infer(head, registry).to_owned())
    };
    let name = match current.and_then(Path::file_name) {
        Some(name) if !outline && existing.is_some() => name.to_string_lossy().into_owned(),
        Some(name) => {
            let mut name = PathBuf::from(name);
            name.set_extension(&extension);
            name.to_string_lossy().into_owned()
        }
        None => format!("無題.{extension}"),
    };
    Suggestion { name, extension }
}

pub(super) fn configure_dialog(dialog: &IFileSaveDialog, suggestion: &Suggestion) -> Result<()> {
    let title = HSTRING::from(format!(
        "{} ファイル (*.{})",
        suggestion.extension.to_uppercase(),
        suggestion.extension
    ));
    let pattern = HSTRING::from(format!("*.{}", suggestion.extension));
    let all = COMDLG_FILTERSPEC {
        pszName: w!("すべてのファイル (*.*)"),
        pszSpec: w!("*.*"),
    };
    let filters = [
        COMDLG_FILTERSPEC {
            pszName: windows::core::PCWSTR(title.as_ptr()),
            pszSpec: windows::core::PCWSTR(pattern.as_ptr()),
        },
        all,
    ];
    unsafe {
        dialog.SetFileTypes(if suggestion.extension.is_empty() {
            std::slice::from_ref(&all)
        } else {
            &filters
        })?;
        dialog.SetFileTypeIndex(1)?;
        dialog.SetDefaultExtension(&HSTRING::from(&suggestion.extension))?;
        dialog.SetFileName(&HSTRING::from(&suggestion.name))
    }
}

fn extension_for_syntax(id: &str) -> Option<&'static str> {
    Some(match id {
        "rust" => "rs",
        "python" => "py",
        "javascript" => "js",
        "typescript" => "ts",
        "csharp" => "cs",
        "cpp" => "cpp",
        "java" => "java",
        "c" => "c",
        "go" => "go",
        "ruby" => "rb",
        "perl" => "pl",
        "php" => "php",
        "vb" => "vb",
        "kotlin" => "kt",
        "swift" => "swift",
        "shell" => "sh",
        "powershell" => "ps1",
        "batch" => "bat",
        "markdown" => "md",
        "json" => "json",
        "html" => "html",
        "xml" => "xml",
        "css" => "css",
        "yaml" => "yaml",
        "toml" => "toml",
        "ini" => "ini",
        "sql" => "sql",
        "cobol" | "cobol-free" => "cbl",
        "jcl" => "jcl",
        "rhai" => "rhai",
        "pli" => "pli",
        "rpg" => "rpg",
        "log" => "log",
        "diff" => "diff",
        _ => return None,
    })
}

fn infer(head: &[u8], registry: &yy_core::syntax::Registry) -> &'static str {
    let head = &head[..head.len().min(SAMPLE_BYTES as usize)];
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{feff}').trim();
    let lower = text.to_ascii_lowercase();
    let root = if lower.starts_with("<?xml") {
        lower
            .split_once("?>")
            .map_or(lower.as_str(), |(_, rest)| rest.trim_start())
    } else {
        &lower
    };
    if root.starts_with("<svg") {
        return "svg";
    }
    if let Some(ext) = registry
        .detect(None, text.as_bytes())
        .as_deref()
        .and_then(extension_for_syntax)
    {
        return ext;
    }
    if text.is_empty() {
        return "txt";
    }
    if text.starts_with(['{', '[']) {
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(_) => return "json",
            Err(error) if error.is_eof() => return "json",
            _ => {}
        }
    }
    let lines: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(128)
        .collect();
    if lower.starts_with("<!doctype html")
        || lower.starts_with("<html")
        || lower.starts_with("<head>")
        || lower.starts_with("<body")
        || lower.starts_with("<div")
        || lower.starts_with("<p>")
        || lower.starts_with("<section")
    {
        return "html";
    }
    if text.starts_with("<?php") {
        return "php";
    }
    if text.starts_with("<?xml")
        || text.starts_with("<!--")
        || (text.starts_with('<') && text.contains("</"))
        || text.strip_prefix('<').is_some_and(|rest| {
            rest.starts_with(|c: char| c.is_alphabetic() || c == '_') && rest.contains("/>")
        })
    {
        return "xml";
    }
    if lines.len() >= 2
        && lines
            .iter()
            .all(|l| l.starts_with('{') && serde_json::from_str::<serde_json::Value>(l).is_ok())
    {
        return "jsonl";
    }
    // Code inside a Markdown fence must not determine the document's extension.
    if lines
        .iter()
        .any(|line| line.starts_with("```") || line.starts_with("~~~"))
    {
        return "md";
    }
    // Strong language markers precede generic Markdown/list/config syntax.
    for line in &lines {
        if line.starts_with("<?php") {
            return "php";
        }
        if line.starts_with("@echo off") || line.starts_with("@ECHO OFF") {
            return "bat";
        }
        if line.starts_with("using System")
            || line.starts_with("namespace ") && text.contains("public class ")
        {
            return "cs";
        }
        if line.starts_with("#include ") || line.starts_with("#include<") {
            return if text.contains("std::") || text.contains("iostream") {
                "cpp"
            } else {
                "c"
            };
        }
        if line.starts_with("fn ")
            || line.starts_with("pub fn ")
            || line.starts_with("use std::")
            || line.starts_with("use crate::")
        {
            return "rs";
        }
        if line.starts_with("package main") || line.starts_with("func ") {
            return "go";
        }
        if line.starts_with("def ") && line.ends_with(':')
            || line.starts_with("from ") && line.contains(" import ")
            || line.starts_with("if __name__ ==")
            || line.starts_with("print(")
            || line.starts_with("import ")
                && !line.ends_with(';')
                && !line.contains(['\'', '"', '{'])
        {
            return "py";
        }
        if line.starts_with("public class ")
            || line.starts_with("import java.")
            || line.starts_with("package ") && line.ends_with(';')
        {
            return "java";
        }
        if line.starts_with("interface ")
            || line.starts_with("export interface ")
            || line.starts_with("type ") && line.contains(" = ")
        {
            return "ts";
        }
        if line.starts_with("function ")
            || line.starts_with("export default ")
            || line.starts_with("const ")
            || line.starts_with("let ") && line.contains('=')
        {
            return "js";
        }
        if line.starts_with("IDENTIFICATION DIVISION")
            || line.starts_with("000100 IDENTIFICATION DIVISION")
        {
            return "cbl";
        }
        let sql = line.to_ascii_uppercase();
        if [
            "SELECT ",
            "INSERT INTO ",
            "CREATE TABLE ",
            "UPDATE ",
            "DELETE FROM ",
            "WITH ",
        ]
        .iter()
        .any(|prefix| sql.starts_with(prefix))
            && (lower.contains(" from ")
                || lower.contains(" into ")
                || lower.contains("table ")
                || text.ends_with(';'))
        {
            return "sql";
        }
    }
    if lines.iter().any(|l| {
        l.starts_with("# ")
            || l.starts_with("## ")
            || l.starts_with("### ")
            || l.starts_with("- [ ] ")
            || l.starts_with("- [x] ")
            || l.contains("](") && l.contains('[')
    }) {
        return "md";
    }
    if lines.len() >= 2 && (lines[1].chars().all(|c| c == '=' || c == '-') && lines[1].len() >= 3) {
        return "md";
    }
    if delimited(text, b'\t') {
        return "tsv";
    }
    if delimited(text, b',') {
        return "csv";
    }
    if lines.iter().filter(|l| assignment(l).is_some()).count() >= 2
        || lines.iter().any(|l| assignment(l).is_some())
            && lines.iter().any(|l| l.starts_with('[') && l.ends_with(']'))
    {
        return if lines.iter().any(|l| {
            assignment(l).is_some_and(|(_, v)| {
                v.trim().starts_with(['"', '\'', '[', '{'])
                    || v.trim().parse::<f64>().is_ok()
                    || matches!(v.trim(), "true" | "false")
            })
        }) {
            "toml"
        } else {
            "ini"
        };
    }
    if text.starts_with("---\n")
        || text.starts_with("---\r\n")
        || lines
            .iter()
            .filter(|line| {
                line.split_once(": ")
                    .is_some_and(|(key, _)| !key.is_empty() && !key.contains([' ', '/', '\\']))
            })
            .count()
            >= 2
    {
        return "yaml";
    }
    if lines
        .iter()
        .filter(|l| {
            l.starts_with("- ")
                || l.starts_with("* ")
                || l.starts_with("+ ")
                || l.starts_with("> ")
                || l.split_once(". ")
                    .is_some_and(|(n, _)| n.parse::<u32>().is_ok())
        })
        .count()
        >= 2
    {
        return "md";
    }
    if text.contains('{') && text.contains('}') && text.contains(':') && text.contains(';') {
        return "css";
    }
    "txt"
}

fn assignment(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    (!key.is_empty() && !key.contains([' ', ':', '/', '<', '>'])).then_some((key, value))
}

/// Count fields outside quoted cells, including multiline CSV; require two equal rows.
fn delimited(text: &str, delimiter: u8) -> bool {
    let dialect = yy_delimited::Dialect::new(&[delimiter], Some(b'"')).unwrap();
    let mut state = yy_delimited::LineState::default();
    let mut widths = Vec::new();
    for line in text.lines().take(128) {
        let (cells, next) = yy_delimited::split_line(line.as_bytes(), &dialect, state);
        if !next.in_quotes && (!line.is_empty() || state.in_quotes) {
            widths.push(cells.last().map_or(1, |cell| cell.field + 1));
        }
        state = next;
        if widths.len() == 32 {
            break;
        }
    }
    !state.in_quotes
        && widths.len() >= 2
        && widths[0] >= 2
        && widths.iter().all(|n| *n == widths[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_common_document_contents_without_changing_bytes() {
        let registry = yy_core::syntax::Registry::builtin();
        for (source, ext) in [
            ("", "txt"),
            ("日本語の普通のメモ😀\r\n次の行", "txt"),
            ("42", "txt"),
            (r#"{"title":"日本語","nodes":[]}"#, "json"),
            ("\u{feff} [1, 2, 3]", "json"),
            ("{broken}", "txt"),
            ("{\"long\": [1, 2,", "json"),
            ("{\"a\":1}\n{\"a\":2}\n", "jsonl"),
            ("# 見出し\n\n本文", "md"),
            ("```rust\nfn main() {}\n```", "md"),
            ("- item one\n- item two", "md"),
            ("[仕様](https://example.com)", "md"),
            ("<!DOCTYPE html>\n<html><body>Hello</body></html>", "html"),
            ("<?xml version=\"1.0\"?><root/>", "xml"),
            ("<root><item>値</item></root>", "xml"),
            ("<root/>", "xml"),
            ("<?php echo 'hi'; ?><div>text</div>", "php"),
            ("<svg xmlns=\"http://www.w3.org/2000/svg\"/>", "svg"),
            ("name: demo\nversion: 1\n", "yaml"),
            ("---\nitems:\n  - a", "yaml"),
            ("[package]\nname = \"demo\"\nversion = \"1\"", "toml"),
            ("name = \"demo\"\nenabled = true", "toml"),
            ("[server]\nhost=localhost\nuser=alice", "ini"),
            ("name,count\nAlice,2\n", "csv"),
            ("name\tcount\nAlice\t2", "tsv"),
            ("name,note\nAlice,\"line1\nline2\"\n", "csv"),
            ("one, two\nplain text without delimiter", "txt"),
            ("#!/usr/bin/env python3\nprint('Hello')", "py"),
            ("#!/bin/bash\necho hi", "sh"),
            ("def hello():\n    return 1", "py"),
            ("fn main() { println!(\"hi\"); }", "rs"),
            ("const value = 1;", "js"),
            ("export interface User { name: string }", "ts"),
            ("#include <iostream>\nint main() { std::cout << 1; }", "cpp"),
            ("#include <stdio.h>\nint main() { return 0; }", "c"),
            ("using System;\npublic class Main {}", "cs"),
            ("import java.util.List;\npublic class Main {}", "java"),
            ("package main\nfunc main() {}", "go"),
            ("@echo off\necho hi", "bat"),
            ("SELECT * FROM users;", "sql"),
            ("body { color: red; }", "css"),
        ] {
            assert_eq!(infer(source.as_bytes(), &registry), ext, "{source}");
        }
        let source = include_str!("../../../../demo/outline/example.yyoutline");
        assert_eq!(infer(source.as_bytes(), &registry), "json");
    }

    #[test]
    fn filenames_preserve_existing_extensions_and_outline_defaults_to_json() {
        let registry = yy_core::syntax::Registry::builtin();
        let choose = |path: Option<&str>, outline, mode, syntax, content: &str| {
            suggest(
                path.map(Path::new),
                outline,
                mode,
                syntax,
                content.as_bytes(),
                &registry,
            )
        };
        assert_eq!(choose(None, true, None, None, "").name, "無題.json");
        assert_eq!(
            choose(Some("notes.txt"), true, None, None, "").name,
            "notes.json"
        );
        assert_eq!(
            choose(Some("日本語.yyoutline"), true, None, None, "").name,
            "日本語.json"
        );
        assert_eq!(
            choose(None, false, None, None, "{\"a\":1}").name,
            "無題.json"
        );
        assert_eq!(
            choose(Some("my file"), false, None, None, "# Title").name,
            "my file.md"
        );
        assert_eq!(
            choose(Some("header.HPP"), false, None, None, "fn main() {}").name,
            "header.HPP"
        );
        assert_eq!(
            choose(Some("notes.txt"), false, None, None, "{\"a\":1}").name,
            "notes.txt"
        );
        assert_eq!(choose(None, false, Some("tsv"), None, "").extension, "tsv");
        assert_eq!(
            choose(None, false, Some("md"), Some("json"), "").extension,
            "md"
        );
        assert_eq!(choose(None, false, None, Some("cpp"), "").extension, "cpp");
        assert_eq!(
            choose(None, false, None, Some("typescript"), "").extension,
            "ts"
        );
        for path in ["Makefile", "Dockerfile", ".gitignore", ".env", ".bashrc"] {
            let result = choose(Some(path), false, None, None, "");
            assert_eq!(result.name, path);
            assert!(result.extension.is_empty());
        }
        assert_eq!(
            choose(None, false, None, None, "# vim: ft=makefile\n").extension,
            "md"
        );
        let huge = format!("{}\nfn main() {{}}", "a".repeat(SAMPLE_BYTES as usize));
        assert_eq!(infer(huge.as_bytes(), &registry), "txt");
    }

    #[test]
    fn native_save_dialog_uses_suggested_filename_and_filter() {
        unsafe {
            use windows::Win32::System::Com::{
                COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
            };
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            struct Com;
            impl Drop for Com {
                fn drop(&mut self) {
                    unsafe {
                        CoUninitialize();
                    }
                }
            }
            let _com = Com;
            for (name, ext) in [
                ("無題.json", "json"),
                ("無題.md", "md"),
                ("table.tsv", "tsv"),
                ("Makefile", ""),
            ] {
                let dialog: IFileSaveDialog =
                    CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).unwrap();
                configure_dialog(
                    &dialog,
                    &Suggestion {
                        name: name.into(),
                        extension: ext.into(),
                    },
                )
                .unwrap();
                let actual = dialog.GetFileName().unwrap();
                let text = actual.to_string().unwrap();
                CoTaskMemFree(Some(actual.0.cast()));
                assert_eq!(text, name);
                assert_eq!(dialog.GetFileTypeIndex().unwrap(), 1);
            }
        }
    }
}
