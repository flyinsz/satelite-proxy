/** Proxy-chain view — the "代理链" grouping of the Nodes page.
 *
 *  Same ownership split as PoolsView: the page owns the current selection /
 *  busy / error plumbing (arrive as props), this component owns the chain list
 *  plus create / edit / delete (reusing ChainPage's editor + menu).
 *
 *  Chains are lighter than pools (no member drawer), so this is a flat row
 *  list — each row shows the hop path, a "使用" action, and a ⋮ menu
 *  (edit / delete). Selecting a chain routes the main group to the chain's
 *  exit-hop outbound (sing-box / mihomo only). */

import { useCallback, useEffect, useState } from "react";
import {
  deleteChain,
  generateSingboxConfig,
  getProxyStatus,
  listChains,
  selectChain,
} from "../api";
import { useI18n } from "../i18n";
import { GlassButton } from "../components/GlassButton";
import { waitForCoreRestart } from "../coreBusy";
import { NODE_GROUP_H, NODE_LIST_COLS } from "../nodeLayout";
import { ChainEditorModal, RowMenu } from "./ChainPage";
import type { AutoSelectMode, NodePool, ProxyChain, ProxyNode } from "../types";

export function ChainsView({
  pools,
  nodes,
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
  /** Pools referenced by chain hops (for hop name resolution + editor). */
  pools: NodePool[];
  nodes: ProxyNode[];
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
  const [openMenuId, setOpenMenuId] = useState<string | null>(null);
  const [editor, setEditor] = useState<{ chain: ProxyChain | null } | null>(null);

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

  async function onDeleteChain(chainId: string) {
    try {
      await deleteChain(chainId);
      setChains((prev) => prev.filter((c) => c.id !== chainId));
      if (currentId === chainId) setCurrentId(null);
    } catch (e) {
      onError(typeof e === "string" ? e : String(e));
    }
  }

  if (loading) {
    return <div className="empty">{t("common.loading")}</div>;
  }

  return (
    <>
      {chains.length === 0 ? (
        <div className="empty card muted">{t("nodes.chainsEmpty")}</div>
      ) : (
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
                  <RowMenu
                    id={`chain-${c.id}`}
                    openId={openMenuId}
                    setOpenId={setOpenMenuId}
                    items={[
                      { key: "use", label: t("nodes.chainUse"), onClick: () => void onUseChain(c.id) },
                      { key: "edit", label: t("common.edit"), onClick: () => setEditor({ chain: c }) },
                      {
                        key: "delete",
                        label: t("common.delete"),
                        danger: true,
                        onClick: () => void onDeleteChain(c.id),
                      },
                    ]}
                  />
                </div>
              );
            })}
          </div>
        </div>
      )}

      <div className="pool-create-row">
        <GlassButton onClick={() => setEditor({ chain: null })}>
          {t("common.create")}
        </GlassButton>
      </div>

      {editor && (
        <ChainEditorModal
          chain={editor.chain}
          nodes={nodes}
          pools={pools}
          onClose={() => setEditor(null)}
          onSaved={() => {
            setEditor(null);
            void reload();
          }}
        />
      )}
    </>
  );
}
