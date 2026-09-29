/** Node-pool view — the "节点池" grouping of the Nodes page.
 *
 *  Lives in its own module rather than inline in NodesPage because the whole
 *  feature is a fork delta on top of upstream: keeping it out of the upstream
 *  file means upstream edits to the node list / grid JSX cannot collide with
 *  it, and merging an upstream release only has to keep a single mount line.
 *
 *  Ownership split:
 *   - NodesPage owns what the page also uses elsewhere: the pools list, the
 *     fold (collapse/expand) buttons' target state, the current selection and
 *     the shared busy/error plumbing. Those arrive as props ("controlled").
 *   - This component owns everything pool-only: the open ⋮ menu, each pool's
 *     effective node (`now`), in-flight per-pool tests and the pool editor. */

import { useCallback, useEffect, useState, type Dispatch, type ReactNode, type SetStateAction } from "react";
import {
  deletePool,
  generateSingboxConfig,
  getPoolActiveNode,
  getProxyStatus,
  selectPool,
  testNodesLatency,
} from "../api";
import { useI18n } from "../i18n";
import { nodeTip } from "../nodeTooltip";
import { waitForCoreRestart } from "../coreBusy";
import { NODE_GROUP_H, NODE_LIST_COLS } from "../nodeLayout";
import { PoolEditorModal, RowMenu } from "./ChainPage";
import type { AutoSelectMode, NodePool, ProxyNode, ViewMode } from "../types";

export function PoolsView({
  pools,
  setPools,
  expanded,
  setExpanded,
  nodes,
  setNodes,
  viewMode,
  currentId,
  setCurrentId,
  autoSelect,
  setAutoSelect,
  busyId,
  setBusyId,
  switching,
  setSwitching,
  testing,
  reload,
  onError,
  renderNodeCard,
}: {
  pools: NodePool[];
  setPools: Dispatch<SetStateAction<NodePool[]>>;
  /** Pool id set — expanded drawers (owned by the page's fold buttons). */
  expanded: Set<string>;
  setExpanded: Dispatch<SetStateAction<Set<string>>>;
  nodes: ProxyNode[];
  setNodes: Dispatch<SetStateAction<ProxyNode[]>>;
  viewMode: ViewMode;
  currentId: string | null;
  setCurrentId: (id: string | null) => void;
  autoSelect: AutoSelectMode;
  setAutoSelect: (mode: AutoSelectMode) => void;
  /** Id of the action in flight (node or pool) — shared busy feedback. */
  busyId: string | null;
  setBusyId: (id: string | null) => void;
  switching: boolean;
  setSwitching: (v: boolean) => void;
  /** A page-wide latency batch is running — blocks the per-pool test. */
  testing: boolean;
  reload: () => Promise<void>;
  /** Set or clear the page's error banner. */
  onError: (message: string | null) => void;
  /** Reuses NodesPage's node card renderer so pool members never drift from
   *  the main grid. `activeOverride` marks the pool's effective node. */
  renderNodeCard: (n: ProxyNode, activeOverride?: string | null) => ReactNode;
}) {
  const { t } = useI18n();
  // One ⋮ menu for every pool row — opening one closes the previous.
  const [openMenuId, setOpenMenuId] = useState<string | null>(null);
  // poolId → currently-effective node id inside that pool (its group `now`).
  const [poolActiveNode, setPoolActiveNode] = useState<Record<string, string | null>>({});
  // poolId set — a latency test for that pool's members is in flight.
  const [poolTestIds, setPoolTestIds] = useState<Set<string>>(new Set());
  const [poolEditor, setPoolEditor] = useState<{ pool: NodePool } | null>(null);

  const togglePoolExpand = useCallback(
    (id: string) => {
      setExpanded((cur) => {
        const next = new Set(cur);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        return next;
      });
    },
    [setExpanded],
  );

  // Resolve the CURRENT pool's effective node so it can be highlighted inside
  // the member list. Only the pool actually in use (current_node_id) gets a
  // live "now" highlight — other pools just show their members without a
  // forced selection marker.
  //
  // Driven by state rather than by the expand click: this component unmounts
  // whenever the user leaves the pools grouping, and on return the drawer is
  // already expanded — a click-driven lookup would never fire and the
  // highlight would silently vanish.
  useEffect(() => {
    if (!currentId) return;
    if (!pools.some((p) => p.id === currentId)) return;
    if (!expanded.has(currentId)) return;
    let cancelled = false;
    getPoolActiveNode(currentId)
      .then((nodeId) => {
        if (!cancelled) setPoolActiveNode((prev) => ({ ...prev, [currentId]: nodeId }));
      })
      .catch(() => {
        if (!cancelled) setPoolActiveNode((prev) => ({ ...prev, [currentId]: null }));
      });
    return () => {
      cancelled = true;
    };
  }, [currentId, expanded, pools]);

  // Selecting a plain node (or nothing) clears the pool "now" highlight; the
  // page no longer has to know that this view keeps such a highlight at all.
  useEffect(() => {
    if (currentId && pools.some((p) => p.id === currentId)) return;
    setPoolActiveNode((prev) => (Object.keys(prev).length === 0 ? prev : {}));
  }, [currentId, pools]);

  // Resolve pool member nodes for the drawer view.
  const poolMembers = useCallback(
    (pool: NodePool): ProxyNode[] => {
      if (pool.mode.mode === "explicit") {
        const ids = pool.mode.node_ids;
        return nodes.filter((n) => ids.includes(n.id));
      }
      // keyword mode
      const { include, exclude } = pool.mode;
      return nodes.filter((n) => {
        const name = n.name.toLowerCase();
        const inc = include.length === 0 || include.some((k) => name.includes(k.toLowerCase()));
        const exc = exclude.length > 0 && exclude.some((k) => name.includes(k.toLowerCase()));
        return inc && !exc;
      });
    },
    [nodes],
  );

  /** Select a pool as the current manual egress (pool's selector outbound). */
  async function onUsePool(poolId: string) {
    if (busyId || switching) return;
    setBusyId(poolId);
    onError(null);
    try {
      const leavingKernel = autoSelect === "kernel";
      await selectPool(poolId);
      setCurrentId(poolId);
      setAutoSelect("off");
      const status = await getProxyStatus().catch(() => null);
      if (!status?.running) {
        await generateSingboxConfig();
      } else if (leavingKernel) {
        setSwitching(true);
        await waitForCoreRestart();
      }
      // Refresh this pool's effective node highlight after switching.
      const activeNodeId = await getPoolActiveNode(poolId).catch(() => null);
      // Only the newly-selected pool keeps a live highlight.
      setPoolActiveNode({ [poolId]: activeNodeId });
    } catch (e) {
      onError(typeof e === "string" ? e : String(e));
    } finally {
      setSwitching(false);
      setBusyId(null);
    }
  }

  /** Latency-test every member of one pool (streaming, same path as the
   *  toolbar test button). Results land back in the node list, so the pool
   *  row's min-latency badge updates automatically. */
  async function onTestPool(poolId: string) {
    if (poolTestIds.has(poolId) || testing) return;
    const pool = pools.find((p) => p.id === poolId);
    if (!pool) return;
    const members = poolMembers(pool);
    if (members.length === 0) return;
    setPoolTestIds((prev) => new Set(prev).add(poolId));
    onError(null);
    const ids = members.map((n) => n.id);
    const idSet = new Set(ids);
    setNodes((prev) =>
      prev.map((n) =>
        idSet.has(n.id) ? { ...n, latency_ms: undefined, latency_at: undefined } : n,
      ),
    );
    try {
      const batch = await testNodesLatency(ids, 3000, () => {});
      // Direct results — apply immediately (streaming callback disabled above
      // to keep this simple; the returned batch has everything).
      setNodes((prev) =>
        prev.map((n) => {
          const r = batch.results.find((r) => r.id === n.id);
          if (!r) return n;
          return { ...n, latency_ms: r.latency_ms ?? null, latency_at: r.tested_at };
        }),
      );
    } catch (e) {
      onError(typeof e === "string" ? e : String(e));
    } finally {
      setPoolTestIds((prev) => {
        const next = new Set(prev);
        next.delete(poolId);
        return next;
      });
      await reload();
    }
  }

  /** Delete a pool (confirm not needed — menu item is explicit). */
  async function onDeletePool(poolId: string) {
    try {
      await deletePool(poolId);
      setPools((prev) => prev.filter((p) => p.id !== poolId));
      if (currentId === poolId) setCurrentId(null);
    } catch (e) {
      onError(typeof e === "string" ? e : String(e));
    }
  }

  return (
    <>
      {pools.length === 0 ? (
        <div className="empty card muted">{t("nodes.poolsEmpty")}</div>
      ) : (
        <div className="card table-wrap">
          <div className="node-list">
            <div className="node-list-head" style={{ gridTemplateColumns: NODE_LIST_COLS }}>
              <span></span>
              <span>{t("nodes.sortName")}</span>
              <span>proto</span>
              <span>host</span>
              <span>port</span>
              <span>{t("nodes.sortLatency")}</span>
            </div>
            {pools.map((p) => {
              const isExpanded = expanded.has(p.id);
              const members = poolMembers(p);
              const isCurrentPool = p.id === currentId;
              const activeMember = isCurrentPool ? (poolActiveNode[p.id] ?? null) : null;
              const poolMinLatency =
                members.length === 0
                  ? null
                  : members.reduce((min, n) => {
                      const ms = n.latency_ms;
                      if (ms == null) return min;
                      return min == null ? ms : Math.min(min, ms);
                    }, null as number | null);
              const poolTesting = poolTestIds.has(p.id);
              return (
                <div key={p.id}>
                  <div
                    className={`node-list-group-row${isCurrentPool ? " row-active" : ""}`}
                    style={{ height: NODE_GROUP_H }}
                    onClick={() => togglePoolExpand(p.id)}
                    title={t("nodes.groupToggleHint")}
                  >
                    <span className={`node-group-caret${isExpanded ? "" : " closed"}`} />
                    <span className="node-group-label">
                      {isCurrentPool ? <span style={{ marginRight: 4 }}>●</span> : null}
                      {p.name}
                    </span>
                    <span className="pool-strategy">
                      {p.strategy === "select" ? t("chain.strategySelect")
                        : p.strategy === "url_test" ? t("chain.strategyUrlTest")
                        : p.strategy === "fallback" ? t("chain.strategyFallback")
                        : p.strategy === "load_balance" ? t("chain.strategyLoadBalance")
                        : p.strategy ?? "—"}
                    </span>
                    <span className="node-group-count mono">
                      {p.mode.mode === "explicit" ? p.mode.node_ids.length : members.length}
                    </span>
                    <span style={{ flex: 1 }} />
                    <span className="pool-latency mono" title={t("nodes.poolMinLatencyHint")}>
                      {poolTesting ? "…" : poolMinLatency != null ? `${poolMinLatency}ms` : "—"}
                    </span>
                    <RowMenu
                      id={`pool-${p.id}`}
                      openId={openMenuId}
                      setOpenId={setOpenMenuId}
                      flipUp={false}
                      items={[
                        { key: "use", label: t("chain.poolUse"), onClick: () => void onUsePool(p.id) },
                        {
                          key: "test",
                          label: t("chain.poolTestLatency"),
                          onClick: () => void onTestPool(p.id),
                        },
                        { key: "edit", label: t("common.edit"), onClick: () => setPoolEditor({ pool: p }) },
                        {
                          key: "delete",
                          label: t("common.delete"),
                          danger: true,
                          onClick: () => void onDeletePool(p.id),
                        },
                      ]}
                    />
                  </div>
                  {isExpanded &&
                    (members.length === 0 ? (
                      <div className="muted" style={{ padding: "0.5rem 0.75rem", fontSize: 12 }}>
                        {t("chain.noPoolMembers")}
                      </div>
                    ) : viewMode === "grid" ? (
                      <div className="node-grid node-grid-pools">
                        {members.map((n) => renderNodeCard(n, activeMember))}
                      </div>
                    ) : (
                      members.map((n) => (
                        <div
                          key={n.id}
                          className={`node-list-row${n.id === currentId || n.id === activeMember ? " row-active" : ""}`}
                          style={{ gridTemplateColumns: NODE_LIST_COLS }}
                          {...nodeTip(n, t)}
                        >
                          <span></span>
                          <span>{n.name}</span>
                          <span>{n.protocol}</span>
                          <span>{n.server}</span>
                          <span>{n.port}</span>
                          <span>
                            {n.latency_ms != null ? `${n.latency_ms}ms` : "—"}
                          </span>
                        </div>
                      ))
                    ))}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {poolEditor && (
        <PoolEditorModal
          pool={poolEditor.pool}
          nodes={nodes}
          onClose={() => setPoolEditor(null)}
          onSaved={() => {
            setPoolEditor(null);
            void reload();
          }}
        />
      )}
    </>
  );
}