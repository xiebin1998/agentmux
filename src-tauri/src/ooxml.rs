//! 最小 xlsx 读取：把第一张工作表转成 CSV。
//!
//! 为什么需要它：回复链路给 Agent 的工具是只读的 `Read/Grep/Glob`，而 `Read`
//! 明确拒绝二进制文件（实测报 `This tool cannot read binary files. The file
//! appears to be a binary .xlsx file.`）。所以 xlsx 必须**由应用侧**先转成文本，
//! 而不是给 Agent 放开执行能力 —— 一个对着任何人发来的消息自动回复的机器人，
//! 不该同时拿到本机代码执行权。
//!
//! 只做一件事：取第一张表的可见文本。样式、公式计算、多表、日期格式都不管。

/// 转换结果的长度上限（字符）。xlsx 可能很大，转出的 CSV 会撑爆 Agent 的上下文，
/// 所以到顶就在行边界截断并显式标注。
pub const MAX_CSV_CHARS: usize = 120_000;

use std::io::Read;

use quick_xml::events::Event;
use quick_xml::Reader;

/// 一个打开的 xlsx 包（xlsx 本质是 zip）。
type Archive<'a> = zip::ZipArchive<std::io::Cursor<&'a [u8]>>;

/// 读包里的一个部件为文本；不存在或读不动都返回 None，由调用方决定兜底。
fn read_entry(zip: &mut Archive<'_>, name: &str) -> Option<String> {
    let mut file = zip.by_name(name).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    Some(text)
}

/// 把 xlsx 的字节转成 CSV 文本（第一张工作表）。
pub fn xlsx_to_csv(bytes: &[u8]) -> anyhow::Result<String> {
    if bytes.is_empty() {
        anyhow::bail!("内容为空，不是 xlsx");
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|err| anyhow::anyhow!("不是合法的 xlsx（zip 打不开）: {err}"))?;

    let shared = read_entry(&mut zip, "xl/sharedStrings.xml")
        .map(|xml| parse_shared_strings(&xml))
        .unwrap_or_default();

    let sheet_path = first_sheet_path(&mut zip);
    let sheet = read_entry(&mut zip, &sheet_path)
        .ok_or_else(|| anyhow::anyhow!("xlsx 里找不到工作表部件: {sheet_path}"))?;

    Ok(render_csv(parse_rows(&sheet, &shared)))
}

/// 第一张工作表在包里的部件路径。
///
/// 不写死 `sheet1.xml`：第一张工作表未必叫 sheet1（删过重建就会变），要走
/// `workbook.xml` 的 r:id → `workbook.xml.rels` 的 Target 去认。任一步缺失就退回
/// 常规名字 —— 那是最常见的布局，不该因为缺少 rels 就整条失败。
fn first_sheet_path(zip: &mut Archive<'_>) -> String {
    const FALLBACK: &str = "xl/worksheets/sheet1.xml";

    let Some(rel_id) = read_entry(zip, "xl/workbook.xml").and_then(|xml| first_sheet_rel_id(&xml))
    else {
        return FALLBACK.to_string();
    };
    let Some(target) =
        read_entry(zip, "xl/_rels/workbook.xml.rels").and_then(|xml| relationship_target(&xml, &rel_id))
    else {
        return FALLBACK.to_string();
    };

    if let Some(absolute) = target.strip_prefix('/') {
        absolute.to_string()
    } else {
        format!("xl/{}", target.trim_start_matches("./"))
    }
}

/// `workbook.xml` 里第一张 `<sheet>` 的 `r:id`。
fn first_sheet_rel_id(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                if event.local_name().as_ref() != "sheet" {
                    continue;
                }
                for attr in event.attributes().flatten() {
                    // `r:id` 的 local name 是 `id`；同元素上的 `sheetId` 不会撞名。
                    if attr.key.local_name().as_ref() == "id" {
                        let value = attr.unescape_value().unwrap_or_default();
                        if !value.trim().is_empty() {
                            return Some(value.trim().to_string());
                        }
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// `.rels` 里指定 `Id` 的 `Target`。
fn relationship_target(xml: &str, rel_id: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                if event.local_name().as_ref() != "Relationship" {
                    continue;
                }
                let mut id = None;
                let mut target = None;
                for attr in event.attributes().flatten() {
                    let value = attr.unescape_value().unwrap_or_default().to_string();
                    match attr.key.local_name().as_ref() {
                        "Id" => id = Some(value),
                        "Target" => target = Some(value),
                        _ => {}
                    }
                }
                if id.as_deref() == Some(rel_id) {
                    return target.filter(|t| !t.trim().is_empty());
                }
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// `Event::GeneralRef` 给的是 `&` 与 `;` 之间的内容（`amp`、`#10` …）。
///
/// XML 只预定义了 5 个命名实体，其余是 DTD 自定义的 —— 不认识的**原样保留**，
/// 静默丢掉会悄悄改变单元格内容。
fn resolve_reference(reference: &quick_xml::events::BytesRef<'_>) -> String {
    if reference.is_char_ref() {
        return reference
            .resolve_char_ref()
            .ok()
            .flatten()
            .map(String::from)
            .unwrap_or_default();
    }
    match reference.as_ref() {
        "amp" => "&".to_string(),
        "lt" => "<".to_string(),
        "gt" => ">".to_string(),
        "quot" => "\"".to_string(),
        "apos" => "'".to_string(),
        other => format!("&{other};"),
    }
}

/// 共享字符串表：每个 `<si>` 里的所有 `<t>` 拼起来。
fn parse_shared_strings(xml: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut out = Vec::new();
    let mut current: Option<String> = None;
    let mut in_text = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => match event.local_name().as_ref() {
                "si" => current = Some(String::new()),
                "t" => in_text = true,
                _ => {}
            },
            Ok(Event::Text(text)) if in_text => {
                if let Some(buffer) = current.as_mut() {
                    buffer.push_str(&text);
                }
            }
            Ok(Event::GeneralRef(reference)) if in_text => {
                if let Some(buffer) = current.as_mut() {
                    buffer.push_str(&resolve_reference(&reference));
                }
            }
            Ok(Event::End(event)) => match event.local_name().as_ref() {
                "t" => in_text = false,
                "si" => out.push(current.take().unwrap_or_default()),
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    out
}

/// 单元格里正在收集的是哪种文本。
#[derive(PartialEq, Eq)]
enum Collecting {
    None,
    /// `<v>` —— 共享字符串下标，或数字/公式结果。
    Value,
    /// `<is><t>` —— 内联字符串。
    InlineText,
}

fn parse_rows(xml: &str, shared: &[String]) -> Vec<Vec<String>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Option<Vec<String>> = None;
    let mut column: Option<usize> = None;
    let mut cell_type = String::new();
    let mut value = String::new();
    let mut collecting = Collecting::None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => match event.local_name().as_ref() {
                "row" => row = Some(Vec::new()),
                "c" => {
                    column = None;
                    cell_type.clear();
                    value.clear();
                    collecting = Collecting::None;
                    for attr in event.attributes().flatten() {
                        match attr.key.local_name().as_ref() {
                            // `r="B7"` 给出行内列位置，缺列时靠它补齐，空单元格才不串位
                            "r" => {
                                column = column_index(&attr.unescape_value().unwrap_or_default())
                            }
                            "t" => {
                                cell_type = attr.unescape_value().unwrap_or_default().to_string()
                            }
                            _ => {}
                        }
                    }
                }
                "v" => collecting = Collecting::Value,
                "t" => collecting = Collecting::InlineText,
                _ => {}
            },
            Ok(Event::Text(text)) => {
                if collecting != Collecting::None {
                    value.push_str(&text);
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if collecting != Collecting::None {
                    value.push_str(&resolve_reference(&reference));
                }
            }
            Ok(Event::End(event)) => match event.local_name().as_ref() {
                "v" | "t" => collecting = Collecting::None,
                "c" => {
                    if let Some(row) = row.as_mut() {
                        let text = match cell_type.as_str() {
                            "s" => value
                                .trim()
                                .parse::<usize>()
                                .ok()
                                .and_then(|index| shared.get(index).cloned())
                                .unwrap_or_default(),
                            _ => value.clone(),
                        };
                        let at = column.unwrap_or(row.len());
                        if at < row.len() {
                            row[at] = text;
                        } else {
                            row.resize(at, String::new());
                            row.push(text);
                        }
                    }
                }
                "row" => {
                    if let Some(done) = row.take() {
                        rows.push(done);
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    rows
}

/// `A` → 0、`B` → 1、`AA` → 26。取不到字母就返回 None（交给调用方按顺序补位）。
fn column_index(cell_ref: &str) -> Option<usize> {
    let mut index = 0usize;
    let mut seen = false;
    for ch in cell_ref.chars() {
        if !ch.is_ascii_alphabetic() {
            break;
        }
        index = index * 26 + (ch.to_ascii_uppercase() as usize - 'A' as usize + 1);
        seen = true;
    }
    seen.then(|| index - 1)
}

fn render_csv(rows: Vec<Vec<String>>) -> String {
    let mut out = String::new();
    let mut truncated = false;

    for row in rows {
        let line = row
            .iter()
            .map(|field| csv_field(field))
            .collect::<Vec<_>>()
            .join(",");
        // 按字符数设上限：xlsx 可能很大，转出来的 CSV 会撑爆 Agent 的上下文。
        let extra = line.chars().count() + if out.is_empty() { 0 } else { 1 };
        if out.chars().count() + extra > MAX_CSV_CHARS {
            truncated = true;
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&line);
    }

    if truncated {
        out.push('\n');
        out.push_str("# 已截断：这张表更长，以上只是一部分。");
    }
    out
}

/// RFC4180：含分隔符/引号/换行的字段要加引号，内部引号翻倍。
fn csv_field(text: &str) -> String {
    if text.contains(',') || text.contains('"') || text.contains('\n') || text.contains('\r') {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// 现场构造一个最小 xlsx。**不提交真实业务文件** —— 夹具在测试里现造。
    ///
    /// `sheet_data` 是 `<sheetData>` 的内容；`shared` 是共享字符串表。
    fn build_xlsx(sheet_data: &str, shared: &[&str]) -> Vec<u8> {
        let items: String = shared
            .iter()
            .map(|text| format!("<si><t>{}</t></si>", text))
            .collect();

        let parts: Vec<(&str, String)> = vec![
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>
</Types>"#
                    .to_string(),
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#
                    .to_string(),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
</Relationships>"#
                    .to_string(),
            ),
            (
                "xl/sharedStrings.xml",
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{n}" uniqueCount="{n}">{items}</sst>"#,
                    n = shared.len(),
                    items = items
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{}</sheetData></worksheet>"#,
                    sheet_data
                ),
            ),
        ];

        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = SimpleFileOptions::default();
            for (name, body) in parts {
                writer.start_file(name, options).expect("写 zip 条目");
                writer.write_all(body.as_bytes()).expect("写条目内容");
            }
            writer.finish().expect("收尾 zip");
        }
        buffer.into_inner()
    }

    #[test]
    fn converts_first_sheet_with_shared_strings() {
        let data = r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
                      <row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>42</v></c></row>"#;
        let csv = xlsx_to_csv(&build_xlsx(data, &["用例编号", "标题", "正常"])).expect("应能转换");

        assert_eq!(csv, "用例编号,标题\n正常,42");
    }

    #[test]
    fn keeps_column_positions_when_cells_are_missing() {
        // 只给了 A 和 C：B 是空的，不能把 C 挤到第二列
        let data = r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1" t="s"><v>1</v></c></row>"#;
        let csv = xlsx_to_csv(&build_xlsx(data, &["甲", "乙"])).expect("应能转换");

        assert_eq!(csv, "甲,,乙");
    }

    #[test]
    fn handles_multi_letter_columns() {
        // AA 是第 27 列
        let data = r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="AA1" t="s"><v>1</v></c></row>"#;
        let csv = xlsx_to_csv(&build_xlsx(data, &["首", "第二十七"])).expect("应能转换");

        assert_eq!(csv.split(',').count(), 27, "应补齐到 27 列");
        assert!(csv.starts_with("首,"), "实际: {}", csv);
        assert!(csv.ends_with("第二十七"), "实际: {}", csv);
    }

    #[test]
    fn quotes_fields_containing_separators() {
        let data = r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>"#;
        let csv = xlsx_to_csv(&build_xlsx(data, &["含,逗号", "含\"引号\""])).expect("应能转换");

        assert_eq!(csv, "\"含,逗号\",\"含\"\"引号\"\"\"");
    }

    #[test]
    fn reads_inline_strings_and_numbers() {
        let data = r#"<row r="1"><c r="A1" t="inlineStr"><is><t>内联文本</t></is></c><c r="B1"><v>3.5</v></c></row>"#;
        let csv = xlsx_to_csv(&build_xlsx(data, &[])).expect("应能转换");

        assert_eq!(csv, "内联文本,3.5");
    }

    #[test]
    fn unescapes_xml_entities_in_text() {
        // xlsx 里的 & < > 都是实体形式，转出来必须是原字符
        let data = r#"<row r="1"><c r="A1" t="s"><v>0</v></c></row>"#;
        let csv = xlsx_to_csv(&build_xlsx(data, &["A &amp; B &lt;标签&gt;"])).expect("应能转换");

        assert_eq!(csv, "A & B <标签>");
    }

    #[test]
    fn truncates_long_sheets_at_row_boundary() {
        // 每行 60+ 字符，3000 行远超 12 万上限
        let long = "测".repeat(60);
        let shared = vec!["一", long.as_str()];
        let mut data = String::new();
        for row in 1..=3000 {
            data.push_str(&format!(
                r#"<row r="{row}"><c r="A{row}" t="s"><v>0</v></c><c r="B{row}" t="s"><v>1</v></c></row>"#
            ));
        }

        let csv = xlsx_to_csv(&build_xlsx(&data, &shared)).expect("应能转换");

        assert!(
            csv.chars().count() <= MAX_CSV_CHARS + 200,
            "必须在行边界截断，实际 {} 字符",
            csv.chars().count()
        );
        assert!(csv.contains("已截断"), "要显式标注截断，别让人以为看到了全部");
        assert!(csv.starts_with("一,测"), "截断也要从第一行开始");
    }

    #[test]
    fn rejects_non_xlsx_input() {
        assert!(xlsx_to_csv(b"not a zip at all").is_err());
        assert!(xlsx_to_csv(&[]).is_err());
    }

    /// 真实业务文件校验：夹具之外，还要在一份真 Excel 上跑通。
    ///
    /// 路径从 `AMX_TEST_XLSX` 取（不写死本机路径）；没设就跳过。默认 `#[ignore]`。
    /// 跑：`AMX_TEST_XLSX=<path> cargo test -- --ignored --nocapture real_xlsx_converts_a_business_file`
    #[test]
    #[ignore]
    fn real_xlsx_converts_a_business_file() {
        let Ok(path) = std::env::var("AMX_TEST_XLSX") else {
            return;
        };
        let bytes = std::fs::read(&path).expect("读真实 xlsx");
        let csv = xlsx_to_csv(&bytes).expect("应能转换真实 xlsx");
        let lines: Vec<&str> = csv.lines().collect();

        // 多行单元格（测试步骤这类）被引号包裹后会跨多个**物理行**，所以物理行数
        // 不等于记录数。留个出口把 CSV 落盘，好用真正的 CSV 解析器逐格对照。
        if let Ok(out) = std::env::var("AMX_TEST_XLSX_OUT") {
            std::fs::write(&out, &csv).expect("写 CSV 产物");
            println!("CSV 已写到: {out}");
        }

        println!("文件: {path}");
        println!("原始字节: {}", bytes.len());
        println!("CSV 物理行数: {}", lines.len());
        println!("CSV 字符数: {}", csv.chars().count());
        for line in lines.iter().take(6) {
            let shown: String = line.chars().take(150).collect();
            println!("  | {shown}");
        }

        assert!(!lines.is_empty(), "真实 xlsx 应至少转出一行");
        assert!(csv.contains('测') || csv.contains('用'), "应含中文表头");
    }
}