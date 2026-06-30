use duckdb::Connection;
use std::time::Instant;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    println!("=== LakeMind Data Profiling & Sampling Simulator ===");

    // 1. Initialize a main DuckDB connection (representing our local lakehouse)
    let conn = Connection::open_in_memory()?;
    
    // Install and load the sqlite extension so we can simulate attached external databases
    conn.execute("INSTALL sqlite;", [])?;
    conn.execute("LOAD sqlite;", [])?;

    // 2. Create a mock external database using SQLite to simulate Postgres/MySQL
    let ext_db_path = "external_source_mock.db";
    let ext_conn = rusqlite::Connection::open(ext_db_path)?;

    // Create a mock table with 50,000 rows in the external database
    println!("[External DB] Creating a mock table with 50,000 rows...");
    ext_conn.execute("DROP TABLE IF EXISTS users;", []).unwrap();
    ext_conn.execute(
        "CREATE TABLE users (
            id INTEGER PRIMARY KEY,
            name TEXT,
            age INTEGER,
            role TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );",
        [],
    ).unwrap();

    // Batch insert mock users
    let roles = vec!["admin", "developer", "designer", "analyst", "support"];
    ext_conn.execute("BEGIN TRANSACTION;", []).unwrap();
    for i in 1..=50_000 {
        let name = format!("User_{i}");
        let age = 18 + (i % 60); // ages between 18 and 77
        let role = roles[(i % roles.len()) as usize];
        ext_conn.execute(
            "INSERT INTO users (name, age, role) VALUES (?, ?, ?);",
            rusqlite::params![name, age, role],
        ).unwrap();
    }
    ext_conn.execute("COMMIT;", []).unwrap();
    println!("[External DB] Mock users table successfully built.");

    // 3. Attach the external SQLite database in our DuckDB session
    println!("\n[DuckDB] Attaching external database as 'ext_catalog'...");
    let attach_sql = format!("ATTACH '{}' AS ext_catalog (TYPE sqlite);", ext_db_path);
    conn.execute(&attach_sql, [])?;
    println!("[DuckDB] External database attached successfully!");

    // 4. Benchmark Strategy A: Zero-copy VIEW
    println!("\n[Benchmark] Creating Zero-copy VIEW 's_users_view' representing full remote table...");
    conn.execute("CREATE OR REPLACE VIEW s_users_view AS SELECT * FROM ext_catalog.users;", [])?;

    // Measure executing queries on the View (representing direct remote scans)
    let start_view_count = Instant::now();
    let count_view: i64 = conn.query_row("SELECT count(*) FROM s_users_view;", [], |r| r.get(0))?;
    let elapsed_view_count = start_view_count.elapsed();
    println!("  - VIEW count query: {} rows, took {:.2?}", count_view, elapsed_view_count);

    let start_view_groupby = Instant::now();
    let mut stmt = conn.prepare("SELECT role, count(*), avg(age) FROM s_users_view GROUP BY role ORDER BY count(*) DESC;")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let role: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        let avg_age: f64 = row.get(2)?;
        // Just consume the row
        let _ = (role, count, avg_age);
    }
    let elapsed_view_groupby = start_view_groupby.elapsed();
    println!("  - VIEW aggregation & group-by query took {:.2?}", elapsed_view_groupby);


    // 5. Benchmark Strategy B: Local Materialized Sampling
    let sample_limit = 1000;
    println!("\n[Benchmark] Creating Local Materialized Sample table 's_users_sample' (Limit: {})...", sample_limit);
    
    let start_sample_creation = Instant::now();
    conn.execute(&format!("CREATE OR REPLACE TABLE s_users_sample AS SELECT * FROM ext_catalog.users LIMIT {};", sample_limit), [])?;
    let elapsed_sample_creation = start_sample_creation.elapsed();
    println!("  - Local sample materialization took {:.2?}", elapsed_sample_creation);

    // Measure executing queries on the local sample table
    let start_sample_count = Instant::now();
    let count_sample: i64 = conn.query_row("SELECT count(*) FROM s_users_sample;", [], |r| r.get(0))?;
    let elapsed_sample_count = start_sample_count.elapsed();
    println!("  - SAMPLE count query: {} rows, took {:.2?}", count_sample, elapsed_sample_count);

    let start_sample_groupby = Instant::now();
    let mut stmt = conn.prepare("SELECT role, count(*), avg(age) FROM s_users_sample GROUP BY role ORDER BY count(*) DESC;")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let role: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        let avg_age: f64 = row.get(2)?;
        let _ = (role, count, avg_age);
    }
    let elapsed_sample_groupby = start_sample_groupby.elapsed();
    println!("  - SAMPLE aggregation & group-by query took {:.2?}", elapsed_sample_groupby);


    // 6. Summary Comparison
    println!("\n=== PERFORMANCE GAINS COMPARISON ===");
    println!("- Count Query: VIEW {:.2?} vs SAMPLE {:.2?} (Speedup: {:.1?}x)", 
        elapsed_view_count, elapsed_sample_count, 
        elapsed_view_count.as_secs_f64() / elapsed_sample_count.as_secs_f64()
    );
    println!("- Group-By Query: VIEW {:.2?} vs SAMPLE {:.2?} (Speedup: {:.1?}x)", 
        elapsed_view_groupby, elapsed_sample_groupby,
        elapsed_view_groupby.as_secs_f64() / elapsed_sample_groupby.as_secs_f64()
    );
    println!("\nConclusion: By materializing a local sample of size {}, subsequent exploratory queries run strictly in local DuckDB memory, bypassing network/external DB overheads entirely.", sample_limit);

    // Clean up temporary sqlite file
    let _ = std::fs::remove_file(ext_db_path);

    Ok(())
}
