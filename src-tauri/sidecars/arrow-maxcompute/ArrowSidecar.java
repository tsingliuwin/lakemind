import com.aliyun.odps.Odps;
import com.aliyun.odps.account.AliyunAccount;
import com.aliyun.odps.tunnel.TableTunnel;
import com.aliyun.odps.tunnel.TableTunnel.DownloadSession;
import com.aliyun.odps.data.ArrowRecordReader;
import com.aliyun.odps.thirdparty.org.apache.arrow.memory.BufferAllocator;
import com.aliyun.odps.thirdparty.org.apache.arrow.memory.RootAllocator;
import com.aliyun.odps.thirdparty.org.apache.arrow.vector.VectorSchemaRoot;
import com.aliyun.odps.thirdparty.org.apache.arrow.vector.ipc.ArrowStreamWriter;

import java.io.BufferedOutputStream;
import java.io.OutputStream;

/**
 * MaxCompute Arrow sidecar: bulk-download a table via the ODPS SDK's
 * `TableTunnel` + `ArrowTunnelRecordReader` (no instance-tunnel 10000-row cap),
 * re-serializing each batch as an Arrow IPC STREAM to stdout (consumed by the
 * Rust host's `arrow::ipc::reader::StreamReader` → DuckDB `appender-arrow`).
 * All logs go to stderr.
 *
 *   usage: ArrowSidecar <project.table> [start] [count]
 *   env  : ODPS_ENDPOINT, ODPS_ACCESS_KEY_ID, ODPS_ACCESS_KEY_SECRET,
 *          [ODPS_TUNNEL_ENDPOINT], [ODPS_REGION], [ODPS_PROJECT]
 *
 * `count == 0` (or beyond EOF) means "to end of table". The Rust host launches
 * this with `--add-opens=java.base/java.nio=ALL-UNNAMED ...` and
 * `-XX:MaxDirectMemorySize=8G` (see `external/arrow_sidecar.rs` ADD_OPENS).
 *
 * Memory note: a single session can accumulate ~250 bytes/row of Arrow direct
 * memory; `-XX:MaxDirectMemorySize=8G` covers ~30M rows. Larger tables need
 * windowed pulls (future: per-window allocator + `getRawStream()` bypass) —
 * see `spike/REPORT.md`.
 */
public class ArrowSidecar {
    public static void main(String[] args) throws Exception {
        String endpoint = System.getenv("ODPS_ENDPOINT");
        String akId = System.getenv("ODPS_ACCESS_KEY_ID");
        String akSecret = System.getenv("ODPS_ACCESS_KEY_SECRET");
        String tunnel = System.getenv("ODPS_TUNNEL_ENDPOINT");
        String region = System.getenv("ODPS_REGION");
        if (endpoint == null || akId == null || akSecret == null || args.length < 1) {
            System.err.println("usage: ArrowSidecar <project.table> [start] [count]  "
                + "(env: ODPS_ENDPOINT, ODPS_ACCESS_KEY_ID/SECRET, [ODPS_TUNNEL_ENDPOINT], [ODPS_REGION])");
            System.exit(2);
        }
        String full = args[0];
        String project, table;
        int dot = full.indexOf('.');
        if (dot < 0) { project = System.getenv("ODPS_PROJECT"); table = full; }
        else { project = full.substring(0, dot); table = full.substring(dot + 1); }

        AliyunAccount acct = new AliyunAccount(akId, akSecret);
        if (region != null && !region.isEmpty()) acct.setRegion(region);
        Odps odps = new Odps(acct);
        odps.setEndpoint(endpoint.replaceFirst("/+$", ""));
        odps.setDefaultProject(project);
        if (tunnel != null && !tunnel.isEmpty()) odps.setTunnelEndpoint(tunnel);
        System.err.println("[arrow] project=" + project + " table=" + table);

        TableTunnel t = new TableTunnel(odps);
        long tCreate = System.currentTimeMillis();
        DownloadSession session = t.createDownloadSession(project, table);
        long total = session.getRecordCount();
        long start = 0;
        long count = total;
        if (args.length >= 3) {            // <table> <start> <count>
            start = Long.parseLong(args[1]);
            count = Long.parseLong(args[2]);
        } else if (args.length >= 2) {     // <table> <count>  (from 0, backward-compat)
            count = Long.parseLong(args[1]);
        }
        if (start < 0) start = 0;
        if (count <= 0 || start + count > total) count = total - start;
        System.err.println("[arrow] createDownloadSession ok in "
            + (System.currentTimeMillis() - tCreate) + "ms; recordCount=" + total
            + " start=" + start + " pulling=" + count);

        BufferAllocator alloc = new RootAllocator(Long.MAX_VALUE);
        ArrowRecordReader reader = session.openArrowRecordReader(start, count, alloc);

        OutputStream out = new BufferedOutputStream(System.out, 1 << 20);
        long rows = 0, batches = 0;
        long t0 = System.currentTimeMillis();
        VectorSchemaRoot root = reader.read();
        if (root == null) {
            System.err.println("[arrow] no data; exiting");
            out.flush();
            reader.close();
            return;
        }
        ArrowStreamWriter writer = new ArrowStreamWriter(root, null, out);
        writer.start();
        do {
            rows += root.getRowCount();
            batches++;
            writer.writeBatch();
            if (batches % 50 == 0) {
                System.err.println("[arrow] batches=" + batches + " rows=" + rows
                    + " (" + (rows * 1000L / Math.max(1, System.currentTimeMillis() - t0)) + " rows/s)");
            }
        } while ((root = reader.read()) != null);
        writer.end();
        writer.close();
        out.flush();
        reader.close();
        long elapsed = System.currentTimeMillis() - t0;
        double rps = elapsed > 0 ? rows * 1000.0 / elapsed : 0;
        System.err.println("[arrow] DONE batches=" + batches + " rows=" + rows
            + " elapsed=" + elapsed + "ms  ->  " + String.format("%.0f", rps) + " rows/sec");
    }
}
