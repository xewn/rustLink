use crate::model::response;
use calamine::{Data, Reader, Xlsx};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Cursor, Read},
};

fn workbook_metadata(contents: &[u8]) -> anyhow::Result<(usize, u32)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(contents))?;
    let total: u64 = (0..archive.len())
        .map(|i| archive.by_index(i).map(|f| f.size()).unwrap_or(0))
        .sum();
    anyhow::ensure!(total <= 64 * 1024 * 1024, "Excel解压后超过64MB限制");
    let mut xml = String::new();
    archive
        .by_name("xl/workbook.xml")?
        .read_to_string(&mut xml)?;
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut active = 0;
    let mut ids = vec![];
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Empty(e) | quick_xml::events::Event::Start(e) => {
                for attr in e.attributes() {
                    let a = attr?;
                    if e.local_name().as_ref() == b"workbookView" && a.key.as_ref() == b"activeTab"
                    {
                        active = std::str::from_utf8(&a.value)?.parse()?;
                    }
                    if e.local_name().as_ref() == b"sheet" && a.key.as_ref() == b"r:id" {
                        ids.push(String::from_utf8(a.value.to_vec())?);
                    }
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    let id = ids
        .get(active)
        .ok_or_else(|| anyhow::anyhow!("active worksheet missing"))?;
    xml.clear();
    archive
        .by_name("xl/_rels/workbook.xml.rels")?
        .read_to_string(&mut xml)?;
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut target = None;
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Empty(e) | quick_xml::events::Event::Start(e)
                if e.local_name().as_ref() == b"Relationship" =>
            {
                let mut rid = String::new();
                let mut path = String::new();
                for a in e.attributes() {
                    let a = a?;
                    match a.key.as_ref() {
                        b"Id" => rid = String::from_utf8(a.value.to_vec())?,
                        b"Target" => path = String::from_utf8(a.value.to_vec())?,
                        _ => {}
                    }
                }
                if &rid == id {
                    target = Some(path);
                    break;
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    let target = target.ok_or_else(|| anyhow::anyhow!("worksheet relationship missing"))?;
    let path = if target.starts_with('/') {
        target.trim_start_matches('/').to_string()
    } else {
        format!("xl/{target}")
    };
    xml.clear();
    archive.by_name(&path)?.read_to_string(&mut xml)?;
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut row = 0;
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Empty(e) | quick_xml::events::Event::Start(e)
                if e.local_name().as_ref() == b"dimension" =>
            {
                for a in e.attributes() {
                    let a = a?;
                    if a.key.as_ref() == b"ref" {
                        let value = std::str::from_utf8(&a.value)?;
                        let last = value.rsplit(':').next().unwrap_or("");
                        row = last
                            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
                            .parse()
                            .unwrap_or(0);
                    }
                }
                break;
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    Ok((active, row))
}

fn text(cell: Option<&Data>) -> String {
    cell.map(ToString::to_string)
        .unwrap_or_default()
        .trim()
        .to_string()
}
pub fn import(filename: &str, contents: &[u8]) -> Value {
    let fail = |s: &str| response(false, s, Value::Null);
    if filename.is_empty() {
        return fail("未选择文件");
    }
    let extension = std::path::Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !matches!(extension.as_str(), "xlsx" | "xls") {
        return fail("仅支持 .xlsx 或 .xls 格式的Excel文件");
    }
    let result = (|| -> anyhow::Result<Value> {
        let (active, declared_rows) = workbook_metadata(contents)?;
        let mut workbook = Xlsx::new(Cursor::new(contents))?;
        let sheet = workbook
            .worksheet_range_at(active)
            .ok_or_else(|| anyhow::anyhow!("no worksheet"))??;
        let mut rows = sheet.rows();
        let header = rows.next().unwrap_or(&[]);
        let rows: Vec<_> = rows.collect();
        if rows.is_empty() && declared_rows < 2 {
            return Ok(fail("Excel文件至少需要包含表头和一行数据"));
        }
        if sheet.start().is_some_and(|(row, _)| row > 0) {
            return Ok(fail("Excel表头必须包含\"点位名称\"和\"地址\"列"));
        }
        let mut columns = HashMap::new();
        for (i, cell) in header.iter().enumerate() {
            let key = match text(Some(cell)).to_lowercase().as_str() {
                "点位名称" | "名称" | "name" => "name",
                "地址" | "address" => "address",
                "数据类型" | "类型" | "type" => "type",
                "倍率" | "rate" => "rate",
                "偏移" | "偏移量" | "offset" => "offset",
                "单位" | "unit" => "unit",
                _ => continue,
            };
            columns.insert(key, i);
        }
        if !columns.contains_key("name") || !columns.contains_key("address") {
            return Ok(fail("Excel表头必须包含\"点位名称\"和\"地址\"列"));
        }
        let mut points = vec![];
        for row in rows {
            let get = |key: &str| text(columns.get(key).and_then(|i| row.get(*i)));
            let (name, address) = (get("name"), get("address"));
            if name.is_empty() || address.is_empty() {
                continue;
            }
            let mut kind = get("type").to_lowercase();
            if !matches!(
                kind.as_str(),
                "bool" | "int16" | "uint16" | "int32" | "uint32" | "float" | "double"
            ) {
                kind = "float".into();
            }
            points.push(json!({"name":name,"address":address,"type":kind,"rate":get("rate").parse::<f64>().unwrap_or(1.0),"offset":get("offset").parse::<f64>().unwrap_or(0.0),"unit":get("unit")}));
        }
        if points.is_empty() {
            return Ok(fail("未解析到有效点位数据，请检查Excel格式"));
        }
        Ok(response(
            true,
            format!("成功导入 {} 个点位", points.len()),
            json!({"points":points}),
        ))
    })();
    result.unwrap_or_else(|e| fail(&format!("Excel解析失败: {e}")))
}
