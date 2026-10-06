/** Proxy-chain view — the "代理链" grouping of the Nodes page.
 *
 *  Same ownership split as PoolsView: the page owns the current selection /
 *  busy / error plumbing (arrive as props), this component owns the chain list
 *  and the "use this chain as the current egress" action.
 *
 *  Chains are lighter than pools: they have no member drawer (their "members"
 *  are hops), so this is a flat row list — each row shows the hop path and a
 *  "使用" action. Selecting a chain routes the main group to the chain's
 *  exit-hop outbound (sing-box / mihomo only). */

import { useCallback, useEffect, useState } from "react";
import {
  generateSingboxConfig,
  getProxyStatus,
  listChains,
  selectChain,
} from "../api";
import { useI18n } from "../i18n";
import { GlassButton } from "../components/GlassButton";
import { waitForCoreRestart } from "../coreBusy";
import { NODE_GROUP_H, NODE_LIST_COLS } from "../nodeLayout";
import type { AutoSelectMode, NodePool, ProxyChain } from "../types";

export function ChainsView({
  pools,
  currentId,
  setCurrentId,
  autoSelect,
  setAutoSelect,
  busyId,
  setBusyId,
  switching,
  setSwitching,
  onError,
}: {
  /** Pools referenced by chain hops (for hop name resolution). */
  pools: NodePool[];
  currentId: string | null;
  setCurrentId: (id: string | null) => void;
  autoSelect: AutoSelectMode;
  setAutoSelect: (mode: AutoSelectMode) => void;
  busyId: string | null;
  setBusyId: (id: string | null) => void;
  switching: boolean;
  setSwitching: (v: boolean) => void;
  onError: (message: string | null) => void;
}) {
  const { t } = useI18n();
  const [chains, setChains] = useState<ProxyChain[]>([]);
  const [loading, setLoading] = useState(true);

  const reload = useCallback(async () => {
    try {
      setChains(await listChains());
    } catch (e) {
      onError(typeof e === "string" ? e : String(e));
    } finally {
      setLoading(false);
    }
  }, [onError]);

  useEffect(() => {
    void reload();
  }, [reload]);

  /** Resolve a hop to a display label (pool name, or node-id prefix). */
  const hopLabel = useCallback(
    (hop: ProxyChain["hops"][number]): string => {
      if (hop.kind === "pool") {
        const pool = pools.find((p) => p.id === hop.pool_id);
        return pool?.name ?? "?";
      }
      return `#${hop.node_id.slice(0, 8)}`;
    },
    [pools],
  );

  /** Select a chain as the current manual egress (mirrors PoolsView's onUsePool). */
  async function onUseChain(chainId: string) {
    if (busyId || switching) return;
    setBusyId(chainId);
    onError(null);
    try {
      const leavingKernel = autoSelect === "kernel";
      await selectChain(chainId);
      setCurrentId(chainId);
      setAutoSelect("off");
      const status = await getProxyStatus().catch(() => null);
      if (!status?.running) {
        await generateSingboxConfig();
      } else if (leavingKernel) {
        setSwitching(true);
        await waitForCoreRestart();
      }
    } catch (e) {
      onError(typeof e === "string" ? e : String(e));
    } finally {
      setSwitching(false);
      setBusyId(null);
    }
  }

  if (loading) {
    return <div className="empty">{t("common.loading")}</div>;
  }

  if (chains.length === 0) {
    return <div className="empty card muted">{t("nodes.chainsEmpty")}</div>;
  }

  return (
    <div className="card table-wrap">
      <div className="node-list">
        <div className="node-list-head" style={{ gridTemplateColumns: NODE_LIST_COLS }}>
          <span></span>
          <span>{t("nodes.sortName")}</span>
          <span>{t("nodes.chainHops")}</span>
          <span>{t("nodes.chainPath")}</span>
          <span></span>
          <span></span>
        </div>
        {chains.map((c) => {
          const isCurrent = c.id === currentId;
          const busy = busyId === c.id;
          const path = c.hops.map(hopLabel).join(" → ");
          return (
            <div
              key={c.id}
              className={`node-list-group-row${isCurrent ? " active-group" : ""}`}
              style={{ height: NODE_GROUP_H }}
            >
              <span>
                {isCurrent ? <span className="node-group-active-mark">●</span> : null}
              </span>
              <span className="node-group-label">{c.name}</span>
              <span className="node-group-count mono">{c.hops.length}</span>
              <span className="chain-hop-path" title={path}>
                {path}
              </span>
              <span style={{ flex: 1 }} />
              <GlassButton
                variant={isCurrent ? "primary" : "plain"}
                disabled={busy || switching}
                onClick={() => void onUseChain(c.id)}
              >
                {busy ? "…" : isCurrent ? t("nodes.chainInUse") : t("nodes.chainUse")}
              </GlassButton>
            </div>
          );
        })}
      </div>
    </div>
  );
}
