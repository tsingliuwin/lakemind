import { For, Show } from "solid-js";
import { typeFamily, type SourceTable } from "../lib/types";
import { t } from "../lib/i18n";

/**
 * Right drawer: Schema/column detail inspector for the currently selected
 * SOURCE table. Shows the table's kind, row-count estimate, Hive partition
 * keys, and a clickable list of columns (name + type-colored badge + null).
 *
 * Clicking a column injects `SELECT col FROM "t" LIMIT 1000` into the editor;
 * the Preview button runs `SELECT * FROM "t" LIMIT 50` directly.
 */
export default function RightInspector(props: {
  table: SourceTable | null;
  busy: boolean;
  onInjectSql: (sql: string) => void;
  onPreview: (table: SourceTable) => void;
}) {
  return (
    <aside class="right">
      <Show
        when={props.table}
        fallback={<div class="ri-empty">{t("inspectorEmptyHint")}</div>}
      >
        {(tVal) => (
          <>
            <div class="ri-header">
              <div class="ri-title">
                <span class="kind-badge" data-kind={tVal().kind}>{tVal().kind}</span>
                <span class="ri-name" title={tVal().name}>{tVal().name}</span>
              </div>
              <div class="ri-meta">
                <Show when={tVal().rowCountEstimate != null}>
                  <span class="rows">{formatCount(tVal().rowCountEstimate!)} {t("rowsUnit")}</span>
                </Show>
                <Show when={tVal().rowCountEstimate == null}>
                  <span>— {t("rowsUnit")}</span>
                </Show>
                <span>· {tVal().columns.length} {t("colsUnit")}</span>
              </div>
              <Show when={tVal().partitionKeys.length > 0}>
                <div class="ri-partitions">
                  <For each={tVal().partitionKeys}>
                    {(k) => <span class="part-pill">🗂 {k}</span>}
                  </For>
                </div>
              </Show>
            </div>

            <div class="ri-path" title={tVal().scanPath}>{tVal().scanPath}</div>

            <div class="ri-section-label">{t("fieldsLabel")}</div>
            <div class="ri-cols">
              <For each={tVal().columns}>
                {(c) => (
                  <div
                    class="col-card"
                    title={`SELECT "${c.name}" FROM "${tVal().name}" LIMIT 1000`}
                    onClick={() => props.onInjectSql(`SELECT "${c.name}" FROM "${tVal().name}" LIMIT 1000;`)}
                  >
                    <span class="col-name">{c.name}</span>
                    <Show when={c.null}>
                      <span class="col-null-mark">∅</span>
                    </Show>
                    <span class="type-badge" data-family={typeFamily(c.type)}>
                      {c.type}
                    </span>
                  </div>
                )}
              </For>
            </div>

            <div class="ri-footer">
              <button
                class="preview-btn"
                disabled={props.busy}
                onClick={() => props.onPreview(tVal())}
              >
                {t("previewRowsBtn")}
              </button>
            </div>
          </>
        )}
      </Show>
    </aside>
  );
}

function formatCount(n: number): string {
  if (n >= 1_000_000_000) return (n / 1_000_000_000).toFixed(1) + "B";
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(0) + "K";
  return String(n);
}
