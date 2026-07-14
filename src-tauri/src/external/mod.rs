//! External-database connectivity via Java sidecars.
//!
//! Sidecar DB types (see `db::is_sidecar_db_type`) do NOT use DuckDB ATTACH.
//! Instead they talk to a Java sidecar process:
//!   - `jdbc_sidecar`  — dbx JDBC plugin (stdio JSON-RPC): ad-hoc SQL, list
//!                       tables, connection test. Works for any JDBC driver.
//!   - `arrow_sidecar` — MaxCompute-specific bulk download via the ODPS SDK's
//!                       `TableTunnel` + `ArrowTunnelRecordReader`, emitting an
//!                       Arrow IPC stream consumed by DuckDB's `appender-arrow`.
//!   - `driver_resolver` — resolve vendor JDBC driver JARs (Maven coords) into
//!                          the app-data cache and build the sidecar classpath.
//!
//! See `spike/REPORT.md` for the validated design + measured throughput.

pub mod arrow_sidecar;
pub mod driver_resolver;
pub mod jdbc_sidecar;
pub mod paths;

#[cfg(test)]
mod tests;
