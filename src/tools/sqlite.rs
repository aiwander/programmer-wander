//! SQLite query tool — direct read-only SQLite access

use anyhow::Result;
use serde_json::{json, Value};

pub async fn query(args: Value) -> Result<Value> {
    let db_path = match args.get("db_path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => anyhow::bail!("db_path required"),
    };
    let sql = match args.get("sql").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => anyhow::bail!("sql required"),
    };
    let max_rows = args.get("max_rows").and_then(|v| v.as_u64()).unwrap_or(100) as usize;

    // Safety: only allow SELECT and PRAGMA
    let sql_upper = sql.trim().to_uppercase();
    if !sql_upper.starts_with("SELECT") && !sql_upper.starts_with("PRAGMA") && !sql_upper.starts_with("EXPLAIN") {
        anyhow::bail!("Only SELECT, PRAGMA, and EXPLAIN queries are allowed (read-only)");
    }

    let db_path_owned = db_path.to_string();
    let sql_owned = sql.to_string();

    // rusqlite is not Send, so run in blocking task
    tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open_with_flags(
            &db_path_owned,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ).map_err(|e| anyhow::anyhow!("Cannot open {}: {}", db_path_owned, e))?;

        let mut stmt = conn.prepare(&sql_owned)
            .map_err(|e| anyhow::anyhow!("SQL error: {}", e))?;

        let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let column_count = column_names.len();

        let mut rows = Vec::new();
        let result = stmt.query_map([], |row| {
            let mut obj = serde_json::Map::new();
            for i in 0..column_count {
                let val: Value = match row.get_ref(i) {
                    Ok(rusqlite::types::ValueRef::Null) => Value::Null,
                    Ok(rusqlite::types::ValueRef::Integer(n)) => json!(n),
                    Ok(rusqlite::types::ValueRef::Real(f)) => json!(f),
                    Ok(rusqlite::types::ValueRef::Text(s)) => {
                        json!(std::str::from_utf8(s).unwrap_or("<invalid utf8>"))
                    },
                    Ok(rusqlite::types::ValueRef::Blob(b)) => json!(format!("<blob {} bytes>", b.len())),
                    Err(_) => Value::Null,
                };
                obj.insert(column_names[i].clone(), val);
            }
            Ok(Value::Object(obj))
        }).map_err(|e| anyhow::anyhow!("Query execution failed: {}", e))?;

        for row in result {
            if rows.len() >= max_rows { break; }
            if let Ok(val) = row {
                rows.push(val);
            }
        }

        let truncated = rows.len() >= max_rows;
        Ok(json!({
            "columns": column_names,
            "rows": rows,
            "count": rows.len(),
            "db_path": db_path_owned,
            "truncated": truncated
        }))
    }).await?
}
