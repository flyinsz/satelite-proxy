import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  checkAppUpdate,
  checkCoreUpdate,
  clearGfwlist,
  diagnoseNetwork,
  downloadCore,
  getPacPreview,
  listPacSourcePresets,
  resetCoreToBundled,
  getAppInstallPath,
  getCoreInfo,
  getPacList,
  getPacStatus,
  getProxyStatus,
  getSettings,
  refreshGfwlist,
  refreshPacGroup,
  regenerateApiSecret,
  restartProxy,
  setCoreType,
  updatePacList,
  updatePacSettings,
  updateSettings,
} from "../api";
import { GlassButton } from "../components/GlassButton";
import { useVisibleInterval } from "../hooks/useVisibleInterval";
import { SolidSelect } from "../components/SolidSelect";
import { GlassSeg } from "../components/GlassSeg";
import { GlassSwitchControl } from "../components/GlassSwitchControl";
import { ErrorModal } from "../components/ErrorModal";
import { TrayIconPicker } from "../components/TrayIconPicker";
import { CoreMark } from "../components/CoreMark";
import {
  beginCoreDownload,
  clearCoreDownload,
  setCoreDownloadError,
  useCoreDownloadState,
} from "../coreDownload";
import { useI18n, type Locale, type MessageKey } from "../i18n";
import { ACCENTS, applyGlowToDom, isCustomHexAccent, resolveAccent } from "../theme/accents";
import { AccentColorPickerModal } from "../components/AccentColorPickerModal";
import { useTheme } from "../theme";
import type {
  AppSettings,
  CoreDownloadProgress,
  CoreInfo,
  CoreKind,
  DiagnosticIssue,
  ExtraInbound,
  HeroStyle,
  PacGroup,
  PacList,
  PacSourcePreset,
  PacStatus,
  ThemeId,
} from "../types";
import { RulesPage } from "./RulesPage";
import { ChainPage } from "./ChainPage";
import { DnsPage } from "./DnsPage";
import { HostsPage } from "./HostsPage";

type SettingsTab =
  | "app"
  | "ports"
  | "pac"
  | "rules"
  | "chain"
  | "multiCore"
  | "dns"
  | "hosts"
  | "core";

/** Editable PAC list categories (keys mirror `PacList` fields). */
type PacGroupKey = "domains" | "gfwlist_domains" | "ip_cidrs" | "suffixes" | "regions";

/** Builtin PAC group ids (map to the legacy flat list fields). */
const BUILTIN_PAC_IDS = new Set<string>([
  "domains",
  "gfwlist_domains",
  "ip_cidrs",
  "suffixes",
  "regions",
]);

/** Left-list group view model (rules-page rule-set card). */
interface PacGroupVM {
  id: string;
  label: string;
  readOnly: boolean;
  items: string[];
  placeholder: string;
  enabled: boolean;
  builtin: boolean;
  remoteUrl: string;
  autoUpdate: boolean;
  intervalHours: number | null;
}

const CUSTOM_BLOCKED_TABS = new Set([
  "rules",
  "chain",
  "multiCore",
  "dns",
  "hosts",
]);

/** Repository link shown in the bottom-right corner of the settings page. */
const PROJECT_URL = "https://github.com/zn0wii/satelite-proxy/";
/** Always-latest app release page, opened from the version tab. */
const RELEASES_URL = "https://github.com/zn0wii/satelite-proxy/releases/latest";

// Session-level memory of the latest MANUAL core update check. The network
// check fires only on the per-core "检查" button (2026-09: it used to fire
// for all three cores on every settings-page mount, hammering GitHub on each
// nav switch) — this snapshot just keeps an already-fetched result visible
// across page remounts (key={nav} destroys the state) without any network
// traffic. An entry is dropped as soon as the installed version no longer
// matches the one it was computed against (stale after download/restore).
type CoreLatestSnapshot = {
  local_version: string | null;
  latest_version: string;
  update_available: boolean;
};
const coreLatestSnapshots = new Map<CoreKind, CoreLatestSnapshot>();

// Accent preset names are picked from the i18n catalog rather than
// AccentPreset.name (theme/accents.ts), which is display data only and not
// locale-aware.
const ACCENT_LABEL_KEY: Record<string, MessageKey> = {
  green: "accent.green",
  blue: "accent.blue",
  purple: "accent.purple",
  pink: "accent.pink",
  orange: "accent.orange",
  cyan: "accent.cyan",
};

/** Idle custom swatch: a rainbow ring hinting "pick any colour". Replaced by
 *  the stored custom hex (inline style) once a custom accent is active. */
const CUSTOM_DOT_RAINBOW =
  "conic-gradient(from 90deg, #f66, #fc6, #6c6, #6cd, #66c, #c6c, #f66)";

export function SettingsPage() {
  const { t, locale, setLocale } = useI18n();
  const { theme, setTheme, accent, setAccent, glow, setGlow, heroStyle, setHeroStyle, glassFrost, setGlassFrost } =
    useTheme();
  const [tab, setTab] = useState<SettingsTab>("app");
  const [settings, setSettings] = useState<AppSettings | null>(null);
  /** Custom accent color picker (the extra swatch after the presets). */
  const [accentPickerOpen, setAccentPickerOpen] = useState(false);
  const [glowPickerOpen, setGlowPickerOpen] = useState(false);
  const [mixed, setMixed] = useState("2080");
  /** Main mixed inbound listens on 0.0.0.0 (LAN) instead of 127.0.0.1. */
  const [allowLan, setAllowLan] = useState(false);
  const [api, setApi] = useState("19090");
  /** Gate for the Clash API secret — off by default (no unexplained key for
   * first-run users); the API stays reachable on 127.0.0.1 either way. */
  const [apiSecretEnabled, setApiSecretEnabled] = useState(false);
  const [probe, setProbe] = useState("");
  const [tunStack, setTunStack] = useState("mixed");
  /** IPv6 address on the TUN interface. Off by default — most nodes have no
   * v6 egress and a dual-stack tun makes Chrome prefer AAAA/v6, black-holing
   * every connection. */
  const [tunIpv6, setTunIpv6] = useState(false);
  /** Reject sniffed QUIC (UDP/443) so browsers fall back to TCP. */
  const [blockQuic, setBlockQuic] = useState(false);
  /** Bypass localhost and LAN segments with built-in direct rules. */
  const [bypassLan, setBypassLan] = useState(true);
  /** Xray sidecar base port — committed on blur/Enter (see onCommitSidecarPort). */
  const [sidecarPort, setSidecarPort] = useState("20890");
  /** Extra inbound drafts — applied on card save (needs core restart). */
  const [extra, setExtra] = useState<ExtraInbound[]>([]);
  // Extra-inbound editor modal (add / edit share one form).
  const [inboundOpen, setInboundOpen] = useState(false);
  const [inboundEditId, setInboundEditId] = useState<string | null>(null);
  const [inboundKind, setInboundKind] = useState<"mixed" | "http">("mixed");
  const [inboundPort, setInboundPort] = useState("");
  const [inboundLan, setInboundLan] = useState(false);
  const [inboundError, setInboundError] = useState<string | null>(null);
  const [menuInboundId, setMenuInboundId] = useState<string | null>(null);
  /** Copy-feedback flag for the read-only Clash API secret field. */
  const [secretCopied, setSecretCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** Detection-only network diagnostics (e.g. system DNS bypassing TUN).
   * Re-checked whenever TUN transitions off → on; never auto-applied. */
  const [netDiagnostics, setNetDiagnostics] = useState<DiagnosticIssue[]>([]);

  /** Per-core status (sing-box + Xray + mihomo). */
  const [cores, setCores] = useState<Record<CoreKind, CoreInfo | null>>({
    singbox: null,
    xray: null,
    mihomo: null,
  });
  const [coreCheckingKind, setCoreCheckingKind] = useState<CoreKind | null>(null);
  const [coreProxyAvailable, setCoreProxyAvailable] = useState(false);
  const [sidecarRunning, setSidecarRunning] = useState(false);
  // Per-kernel "include pre-releases" toggle for the update check. Opt-in
  // and unpersisted: the REST API path it enables is rate-limited (60 req/h
  // per IP, unauthenticated), so it must stay a deliberate choice made right
  // before each manual check, not a sticky setting that silently keeps
  // spending that budget.
  const [corePrerelease, setCorePrerelease] = useState<Record<CoreKind, boolean>>({
    singbox: false,
    xray: false,
    mihomo: false,
  });
  // coreError is shared by every core-related flow on this page (update
  // check, reload, download, restore) and feeds the single ErrorModal below.
  const [coreError, setCoreError] = useState<string | null>(null);
  // Download/restore busy-kind additionally mirrors into the global
  // coreDownload store so the floating toast (App.tsx) can show progress —
  // this page itself only needs the busy flag now, the progress bar lives
  // solely in the toast.
  const coreDownloadState = useCoreDownloadState();
  const coreBusyKind = coreDownloadState?.kind ?? null;

  // App's own version card: local version is instant (getVersion), the
  // latest GitHub tag needs a network check that routes via the proxy
  // when the kernel is running (same strategy as the core check).
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [appUpdate, setAppUpdate] = useState<{
    current_version: string;
    latest_version: string;
    update_available: boolean;
    cached: boolean;
    checked_at: number | null;
  } | null>(null);
  const [appChecking, setAppChecking] = useState(false);
  const [appError, setAppError] = useState<string | null>(null);
  /** Absolute path of the app's own executable, shown like the kernel path. */
  const [appPath, setAppPath] = useState<string | null>(null);

  // ---- PAC tab state ---------------------------------------------------
  const [pacStatus, setPacStatus] = useState<PacStatus | null>(null);
  const [pacList, setPacList] = useState<PacList | null>(null);
  const [pacSaving, setPacSaving] = useState(false);
  const [pacRefreshing, setPacRefreshing] = useState(false);
  const [pacError, setPacError] = useState<string | null>(null);
  /** Built-in gfwlist mirrors, loaded once for the source picker. */
  const [pacPresets, setPacPresets] = useState<PacSourcePreset[]>([]);
  /** Source URL draft (committed with "Save settings"). */
  const [pacSourceUrl, setPacSourceUrl] = useState("");
  const [pacAutoUpdate, setPacAutoUpdate] = useState(false);
  const [pacIntervalHours, setPacIntervalHours] = useState(24);
  const [pacPort, setPacPort] = useState("");
  const [pacSettingsSaving, setPacSettingsSaving] = useState(false);
  const [pacClearing, setPacClearing] = useState(false);
  const [pacPreview, setPacPreview] = useState<string | null>(null);
  const [pacPreviewLoading, setPacPreviewLoading] = useState(false);
  const [pacCopied, setPacCopied] = useState(false);
  const pacCopiedTimer = useRef<number | null>(null);
  /** Left list — currently selected group id. */
  const [pacActiveGroup, setPacActiveGroup] = useState("domains");
  /** Draft for the add-entry input in the detail pane. */
  const [pacNewItem, setPacNewItem] = useState("");
  /** Which group's ⋮ menu is open (rules-page style). */
  const [pacMenuGroup, setPacMenuGroup] = useState<string | null>(null);
  /** "New/Edit group" modal — shared by 新建 (id=null) and 编辑 (id set). */
  const [pacEditOpen, setPacEditOpen] = useState(false);
  const [pacEditGroup, setPacEditGroup] = useState<string | null>(null);
  const [pacEditName, setPacEditName] = useState("");
  const [pacEditRemoteUrl, setPacEditRemoteUrl] = useState("");
  const [pacEditAutoUpdate, setPacEditAutoUpdate] = useState(false);
  const [pacEditInterval, setPacEditInterval] = useState(24);
  /** "Port" modal — global PAC service port, reachable from the right toolbar. */
  const [pacPortOpen, setPacPortOpen] = useState(false);

  const reloadPacTab = useCallback(async () => {
    const [status, list] = await Promise.all([
      getPacStatus().catch(() => null),
      getPacList().catch(() => null),
    ]);
    setPacStatus(status);
    if (status) {
      setPacSourceUrl(status.source_url);
      setPacAutoUpdate(status.auto_update);
      setPacIntervalHours(status.update_interval_hours);
    }
    if (list) setPacList(list);
  }, []);

  /** PAC list groups for the left-list / right-detail layout.
      Builtin ids map to the legacy flat fields; custom groups carry items. */
  const pacGroups = useMemo(() => {
    const list = pacList;
    const g = (id: string): PacGroup =>
      list?.groups.find((x) => x.id === id) ?? {
        id,
        name: id,
        enabled: true,
        items: [],
        remote_url: null,
        auto_update: false,
        update_interval_hours: null,
      };
    const vm = (
      id: string,
      label: string,
      readOnly: boolean,
      items: string[],
      placeholder: string,
      builtin: boolean,
    ): PacGroupVM => {
      const grp = g(id);
      // The gfwlist mirror's source lives in the global PAC settings
      // (store pac_source_url / pac_auto_update / interval), not in
      // groups[].remote_url — keep it visible here so the edit modal and
      // the ⋮ refresh entry work for the builtin as well.
      const isGfwlist = id === "gfwlist_domains";
      return {
        id,
        label,
        readOnly,
        items,
        placeholder,
        enabled: grp.enabled,
        builtin,
        remoteUrl: isGfwlist ? (pacSourceUrl || "") : (grp.remote_url ?? ""),
        autoUpdate: isGfwlist ? pacAutoUpdate : grp.auto_update,
        intervalHours: isGfwlist
          ? pacIntervalHours
          : grp.update_interval_hours,
      };
    };
    const builtins: PacGroupVM[] = [
      vm("domains", t("pac.domains"), false, list?.domains ?? [], "google.com", true),
      vm("gfwlist_domains", t("pac.gfwlistDomains"), true, list?.gfwlist_domains ?? [], "", true),
      vm("ip_cidrs", t("pac.ipCidrs"), false, list?.ip_cidrs ?? [], "1.2.3.4 / 10.0.0.0/8", true),
      vm("suffixes", t("pac.suffixes"), false, list?.suffixes ?? [], ".githubusercontent.com", true),
      vm("regions", t("pac.regions"), false, list?.regions ?? [], "US / HK", true),
    ];
    const custom: PacGroupVM[] = (list?.groups ?? [])
      .filter((x) => !BUILTIN_PAC_IDS.has(x.id))
      .map((x) =>
        vm(x.id, x.name, false, x.items, "google.com", false),
      );
    return [...builtins, ...custom];
  }, [pacList, pacSourceUrl, pacAutoUpdate, pacIntervalHours, t]);

  const activePacGroup =
    pacGroups.find((g) => g.id === pacActiveGroup) ?? pacGroups[0];

  /** Patch one group's entries (trim + de-dup, drop comments/blank).
      Builtin ids write back to the legacy flat fields; custom groups write
      into `groups[].items`. */
  const updatePacGroup = useCallback((id: string, items: string[]) => {
    const cleaned = [
      ...new Set(
        items
          .map((s) => s.trim())
          .filter((s) => s.length > 0 && !s.startsWith("#")),
      ),
    ];
    setPacList((prev) => {
      if (!prev) return prev;
      if (BUILTIN_PAC_IDS.has(id as PacGroupKey)) {
        return { ...prev, [id]: cleaned };
      }
      return {
        ...prev,
        groups: prev.groups.map((x) =>
          x.id === id ? { ...x, items: cleaned } : x,
        ),
      };
    });
  }, []);

  /** Toggle a group's enabled flag (persisted inside `groups`). */
  const togglePacGroup = useCallback((id: string, enabled: boolean) => {
    setPacList((prev) => {
      if (!prev) return prev;
      const exists = prev.groups.some((x) => x.id === id);
      const groups = exists
        ? prev.groups.map((x) => (x.id === id ? { ...x, enabled } : x))
        : [
            ...prev.groups,
            {
              id,
              name: id,
              enabled,
              items: [] as string[],
              remote_url: null as string | null,
              auto_update: false,
              update_interval_hours: null as number | null,
            },
          ];
      return { ...prev, groups };
    });
  }, []);

  /** Delete a custom group. Builtin groups are never deletable. */
  const deletePacGroup = useCallback((id: string) => {
    setPacList((prev) => {
      if (!prev || BUILTIN_PAC_IDS.has(id as PacGroupKey)) return prev;
      const groups = prev.groups.filter((x) => x.id !== id);
      return { ...prev, groups };
    });
    setPacActiveGroup((cur) => (cur === id ? "domains" : cur));
  }, []);

  /** Reset all groups to defaults: builtin entries cleared/kept per field,
      custom groups removed, all enabled. */
  const resetPacGroups = useCallback(() => {
    setPacList((prev) => {
      if (!prev) return prev;
      const mk = (id: string): PacGroup => ({
        id,
        name: id,
        enabled: true,
        items: [],
        remote_url: null,
        auto_update: false,
        update_interval_hours: null,
      });
      return {
        ...prev,
        domains: [],
        ip_cidrs: [],
        suffixes: [],
        regions: [],
        groups: [
          mk("domains"),
          mk("gfwlist_domains"),
          mk("ip_cidrs"),
          mk("suffixes"),
          mk("regions"),
        ],
      };
    });
  }, []);

  const addPacEntry = useCallback(() => {
    const value = pacNewItem.trim();
    if (!value) return;
    const id = pacActiveGroup;
    const current = activePacGroup?.items ?? [];
    if (current.includes(value)) {
      setPacNewItem("");
      return;
    }
    updatePacGroup(id, [...current, value]);
    setPacNewItem("");
  }, [pacNewItem, pacActiveGroup, activePacGroup, updatePacGroup]);

  const removePacEntry = useCallback(
    (index: number) => {
      const id = pacActiveGroup;
      const current = activePacGroup?.items ?? [];
      updatePacGroup(
        id,
        current.filter((_, i) => i !== index),
      );
    },
    [pacActiveGroup, activePacGroup, updatePacGroup],
  );

  /** Create or edit a group (id=null → new; id set → update name /
      remote URL / auto-update). Builtin groups keep their fixed labels and
      legacy field mapping, but may still carry a remote URL + auto-update
      (e.g. gfwlist mirror preset). */
  const savePacGroup = useCallback(
    (id: string | null, name: string, remoteUrl: string, autoUpdate: boolean, interval: number) => {
      const trimmed = name.trim();
      if (!trimmed) return;
      const url = remoteUrl.trim();
      const patch: Partial<PacGroup> = {
        remote_url: url || null,
        auto_update: autoUpdate && !!url,
        update_interval_hours: autoUpdate && url ? interval : null,
      };
      setPacList((prev) => {
        if (!prev) return prev;
        if (id === null) {
          const nid = `group-${Date.now()}`;
          const group: PacGroup = {
            id: nid,
            name: trimmed,
            enabled: true,
            items: [],
            remote_url: patch.remote_url ?? null,
            auto_update: patch.auto_update ?? false,
            update_interval_hours: patch.update_interval_hours ?? null,
          };
          setPacActiveGroup(nid);
          return { ...prev, groups: [...prev.groups, group] };
        }
        return {
          ...prev,
          groups: prev.groups.map((x) =>
            x.id === id
              ? {
                  ...x,
                  // Builtin labels are fixed (localized); only custom
                  // groups may be renamed.
                  ...(BUILTIN_PAC_IDS.has(id) ? {} : { name: trimmed }),
                  ...patch,
                }
              : x,
          ),
        };
      });
      setPacEditOpen(false);
      setPacEditName("");
      setPacEditRemoteUrl("");
      setPacEditAutoUpdate(false);
      setPacEditInterval(24);
    },
    [],
  );

  /** Save the edit modal — gfwlist persists its source via the global
      PAC settings (updatePacSettings), every other group via
      savePacGroup (name + groups[].remote_url). */
  const savePacEdit = useCallback(
    async (id: string | null, name: string, remoteUrl: string, autoUpdate: boolean, interval: number) => {
      if (id === "gfwlist_domains") {
        const url = remoteUrl.trim();
        if (!/^https?:\/\//i.test(url)) {
          setPacError(t("pac.sourceInvalid"));
          return;
        }
        setPacSettingsSaving(true);
        setPacError(null);
        try {
          const status = await updatePacSettings({
            sourceUrl: url,
            autoUpdate,
            updateIntervalHours: interval,
            pacPort: pacStatus?.port,
          });
          setPacStatus(status);
          setPacSourceUrl(status.source_url);
          setPacAutoUpdate(status.auto_update);
          setPacIntervalHours(status.update_interval_hours);
          setPacEditOpen(false);
        } catch (e) {
          setPacError(
            t("pac.settingsError", { err: typeof e === "string" ? e : String(e) }),
          );
        } finally {
          setPacSettingsSaving(false);
        }
        return;
      }
      savePacGroup(id, name, remoteUrl, autoUpdate, interval);
    },
    [pacPort, savePacGroup, t],
  );

  const savePacList = useCallback(async () => {
    if (!pacList) return;
    setPacSaving(true);
    setPacError(null);
    try {
      // Entries are normalized on edit; gfwlist stays read-only.
      const saved = await updatePacList(pacList);
      pacSavedRef.current = saved;
      setPacList(saved);
      setPacStatus(await getPacStatus().catch(() => null));
    } catch (e) {
      setPacError(t("pac.saveError", { err: typeof e === "string" ? e : String(e) }));
    } finally {
      setPacSaving(false);
    }
  }, [pacList, t]);

  /** Auto-persist list changes (group toggle / entries / create / rename /
      delete / reset) like the rules page — no explicit save button.
      `pacSavedRef` holds the last server-acknowledged copy so the
      `setPacList(saved)` inside savePacList does not loop back into
      another save (which previously kept pacSaving true forever and made
      the reset button look stuck in a busy state). */
  const pacFirstLoaded = useRef(false);
  const pacSavedRef = useRef<PacList | null>(null);
  useEffect(() => {
    if (!pacList || !pacFirstLoaded.current) {
      pacFirstLoaded.current = !!pacList;
      return;
    }
    if (pacSavedRef.current === pacList) return;
    const tmr = window.setTimeout(() => {
      void savePacList();
    }, 250);
    return () => window.clearTimeout(tmr);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pacList]);

  /** Manual refresh for any group with a remote URL — custom groups go
      through refresh_pac_group; the gfwlist builtin keeps its own command
      (it is special-cased upstream). */
  const onRefreshPacGroup = useCallback(
    async (id: string) => {
      setPacRefreshing(true);
      setPacError(null);
      try {
        if (id === "gfwlist_domains") {
          setPacList(await refreshGfwlist());
        } else {
          setPacList(await refreshPacGroup(id));
        }
        setPacStatus(await getPacStatus().catch(() => null));
      } catch (e) {
        setPacError(
          t("pac.refreshError", { err: typeof e === "string" ? e : String(e) }),
        );
      } finally {
        setPacRefreshing(false);
      }
    },
    [t],
  );

  const onClearGfwlist = useCallback(async () => {
    setPacClearing(true);
    setPacError(null);
    try {
      setPacList(await clearGfwlist());
      setPacStatus(await getPacStatus().catch(() => null));
    } catch (e) {
      setPacError(t("pac.settingsError", { err: typeof e === "string" ? e : String(e) }));
    } finally {
      setPacClearing(false);
    }
  }, [t]);

  const onCopyPac = useCallback(async () => {
    if (pacPreview == null) return;
    try {
      await navigator.clipboard.writeText(pacPreview);
      setPacCopied(true);
      if (pacCopiedTimer.current != null) window.clearTimeout(pacCopiedTimer.current);
      pacCopiedTimer.current = window.setTimeout(() => setPacCopied(false), 1500);
    } catch {
      // Clipboard unavailable — the <pre> stays selectable as the fallback.
    }
  }, [pacPreview]);

  const onPreviewPac = useCallback(async () => {
    setPacPreviewLoading(true);
    setPacPreview("");
    setPacError(null);
    try {
      setPacPreview(await getPacPreview());
    } catch (e) {
      setPacPreview(null);
      setPacError(t("pac.previewError", { err: typeof e === "string" ? e : String(e) }));
    } finally {
      setPacPreviewLoading(false);
    }
  }, [t]);

  const tabs = useMemo(
    () =>
      [
        {
          id: "app" as const,
          label: t("settings.tabApp"),
          hint: t("settings.hintApp"),
        },
        {
          id: "ports" as const,
          label: t("settings.tabPorts"),
          hint: t("settings.hintPorts"),
        },
        {
          id: "pac" as const,
          label: t("settings.tabPac"),
          hint: t("settings.hintPac"),
        },
        {
          id: "rules" as const,
          label: t("settings.tabRules"),
          hint: t("settings.hintRules"),
        },
        {
          id: "chain" as const,
          label: t("settings.tabChain"),
          hint: t("settings.hintChain"),
        },
        {
          id: "multiCore" as const,
          label: t("settings.tabMultiCore"),
          hint: t("settings.hintMultiCore"),
        },
        {
          id: "dns" as const,
          label: t("settings.tabDns"),
          hint: t("settings.hintDns"),
        },
        {
          id: "hosts" as const,
          label: t("settings.tabHosts"),
          hint: t("settings.hintHosts"),
        },
        {
          id: "core" as const,
          label: t("settings.tabCore"),
          hint: t("settings.hintCore"),
        },
      ] as const,
    [t],
  );

  // Manual-only ("检查" button): hits the network on every click. The result
  // is mirrored into the session snapshot so remounts keep showing it.
  const runCoreUpdateCheck = useCallback(
    async (kind: CoreKind, localVersion: string | null, includePrerelease: boolean) => {
      setCoreCheckingKind(kind);
      setCoreError(null);
      try {
        const update = await checkCoreUpdate(kind, localVersion, includePrerelease);
        coreLatestSnapshots.set(kind, {
          local_version: localVersion,
          latest_version: update.latest_version,
          update_available: update.update_available,
        });
        setCores((prev) => {
          const info = prev[kind];
          if (!info) return prev;
          return {
            ...prev,
            [kind]: {
              ...info,
              latest_version: update.latest_version,
              update_available: update.update_available,
            },
          };
        });
      } catch (e) {
        setCoreError(typeof e === "string" ? e : String(e));
      } finally {
        setCoreCheckingKind(null);
      }
    },
    [],
  );

  // Local core status only — no version check here. Latest-release lookups
  // are manual-only (and rate-limited by being click-driven); overlaying the
  // session snapshot below is purely in-memory.
  const reloadCore = useCallback(async () => {
    setCoreError(null);
    try {
      const results = await Promise.all([
        getCoreInfo("singbox"),
        getCoreInfo("xray"),
        getCoreInfo("mihomo"),
      ]);
      const [singbox, xray, mihomo] = results;
      const next: Record<CoreKind, CoreInfo> = { singbox, xray, mihomo };
      for (const kind of Object.keys(next) as CoreKind[]) {
        const snap = coreLatestSnapshots.get(kind);
        if (!snap) continue;
        if (snap.local_version !== next[kind].version) {
          // Binary changed since the check (download/restore) — stale.
          coreLatestSnapshots.delete(kind);
          continue;
        }
        next[kind] = {
          ...next[kind],
          latest_version: snap.latest_version,
          update_available: snap.update_available,
        };
      }
      setCores(next);
    } catch (e) {
      setCoreError(typeof e === "string" ? e : String(e));
    }
  }, []);

  useEffect(() => {
    getSettings()
      .then((s) => {
        setSettings(s);
        setMixed(String(s.mixed_port));
        setAllowLan(!!s.allow_lan);
        setApi(String(s.api_port));
        setApiSecretEnabled(!!s.api_secret_enabled);
        setProbe(s.probe_url);
        setTunStack(s.tun_stack || "mixed");
        setTunIpv6(!!s.tun_ipv6_enabled);
        setBlockQuic(!!s.block_quic);
        setBypassLan(s.bypass_lan !== false);
        setExtra(s.extra_inbounds ?? []);
        setSidecarPort(String(s.sidecar_port ?? 20890));
      })
      .catch((e) => setError(typeof e === "string" ? e : String(e)));
    void reloadCore();
  }, [reloadCore]);

  /** reportError: surface failures (manual click). force: bypass the 6h
   * cache and hit the network — manual checks always refresh, auto checks
   * on tab open reuse a fresh cached result. */
  const runAppUpdateCheck = useCallback(
    async (reportError: boolean, force = false) => {
      setAppChecking(true);
      if (reportError) setAppError(null);
      try {
        setAppUpdate(await checkAppUpdate(force));
      } catch (e) {
        if (reportError) {
          setAppError(typeof e === "string" ? e : String(e));
        }
      } finally {
        setAppChecking(false);
      }
    },
    [],
  );

  useEffect(() => {
    if (tab !== "core") return;
    void getVersion()
      .then(setAppVersion)
      .catch(() => setAppVersion(null));
    void getAppInstallPath()
      .then(setAppPath)
      .catch(() => setAppPath(null));
    void runAppUpdateCheck(false);
  }, [tab, runAppUpdateCheck]);

  // Poll on both the core and multi-core tabs: the multi-core tab's running
  // pill must follow the debounced restart that enabling the switch triggers
  // (stop → regenerate → main core health → sidecar spawn takes a few
  // seconds after the toggle lands).
  useVisibleInterval(
    () =>
      getProxyStatus()
        .then((status) => {
          setCoreProxyAvailable(status.running);
          setSidecarRunning(!!status.sidecar_running);
        })
        .catch(() => {
          setCoreProxyAvailable(false);
          setSidecarRunning(false);
        }),
    tab === "core" || tab === "multiCore" ? 2000 : null,
  );

  // Close the inbound-row ⋮ menu on outside pointer-down / Escape.
  useEffect(() => {
    if (!menuInboundId) return;
    function onDocPointerDown(e: PointerEvent) {
      const t = e.target as HTMLElement | null;
      if (t?.closest?.("[data-inbound-menu]")) return;
      setMenuInboundId(null);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setMenuInboundId(null);
    }
    document.addEventListener("pointerdown", onDocPointerDown, true);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDocPointerDown, true);
      document.removeEventListener("keydown", onKey);
    };
  }, [menuInboundId]);

  // Close the PAC group ⋮ menu on outside pointer-down / Escape, mirroring
  // the rules page's rule-set menu behaviour.
  useEffect(() => {
    if (!pacMenuGroup) return;
    function onDocPointerDown(e: PointerEvent) {
      const t = e.target as HTMLElement | null;
      if (t?.closest?.("[data-pac-menu]")) return;
      setPacMenuGroup(null);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setPacMenuGroup(null);
    }
    document.addEventListener("pointerdown", onDocPointerDown, true);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDocPointerDown, true);
      document.removeEventListener("keydown", onKey);
    };
  }, [pacMenuGroup]);

  useEffect(() => {
    // Settings tabs remount pages often; if this unmounts before listen()
    // resolves, dispose immediately so the listener doesn't leak. Progress
    // itself is tracked globally (coreDownload.ts) so it survives this page
    // unmounting mid-download — this listener only needs the running-status
    // flag for the "core running" badge.
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<CoreDownloadProgress>("core-download-progress", (event) => {
      setCoreProxyAvailable(event.payload.via_proxy);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  /** Latest auto-apply fn (called from the debounced effect and re-queued
   * from its own finally when the user edited mid-flight). */
  const autoApplyRef = useRef<() => Promise<void>>(async () => {});
  const applyingRef = useRef(false);
  /** Bumped by the debounced effect on every draft change; snapshotted at the
   * start of an apply so `finally` can tell "user edited again while this
   * attempt was in flight" apart from "this same attempt just failed and
   * `dirty` is still true because the failed call never landed in `settings`".
   * Without this, a persistently-failing restart (e.g. LAN bind failure)
   * retries itself forever with no backoff. */
  const applyGenerationRef = useRef(0);
  /** Previous tun_enabled value, to detect the off → on transition. */
  const prevTunEnabledRef = useRef<boolean | undefined>(undefined);

  // Re-run detection-only network diagnostics whenever TUN turns on. Never
  // fires on every settings refresh — only on the actual off → on edge —
  // since the check involves a couple of shell-outs on macOS.
  useEffect(() => {
    const wasOn = prevTunEnabledRef.current;
    const isOn = !!settings?.tun_enabled;
    prevTunEnabledRef.current = isOn;
    if (wasOn === undefined || wasOn === isOn || !isOn) {
      if (!isOn) setNetDiagnostics([]);
      return;
    }
    diagnoseNetwork()
      .then((r) => setNetDiagnostics(r.issues))
      .catch(() => setNetDiagnostics([]));
  }, [settings?.tun_enabled]);

  /** Auto-commit the ports tab: save every draft (ports / LAN / probe /
   * stack / listeners) and restart the core when it is running. Drafts that
   * are still invalid (mid-typing) are skipped until they become valid. */
  const autoApplyNetwork = useCallback(async () => {
    if (applyingRef.current || !settings) return;
    const dirty =
      String(settings.mixed_port) !== mixed.trim() ||
      !!settings.allow_lan !== allowLan ||
      String(settings.api_port) !== api.trim() ||
      !!settings.api_secret_enabled !== apiSecretEnabled ||
      (settings.probe_url ?? "") !== probe ||
      (settings.tun_stack || "mixed") !== tunStack ||
      !!settings.tun_ipv6_enabled !== tunIpv6 ||
      !!settings.block_quic !== blockQuic ||
      (settings.bypass_lan !== false) !== bypassLan ||
      !sameInbounds(settings.extra_inbounds ?? [], extra);
    if (!dirty) return;
    // Invalid drafts (mid-typing or left behind): surface why we can't apply
    // yet; the banner clears on the next successful auto-commit.
    const mixedPort = Number(mixed);
    const apiPort = Number(api);
    if (!Number.isFinite(mixedPort) || mixedPort < 1 || mixedPort > 65535) {
      setError(t("settings.invalidMixed"));
      return;
    }
    if (!Number.isFinite(apiPort) || apiPort < 1 || apiPort > 65535) {
      setError(t("settings.invalidApi"));
      return;
    }
    const seen = new Set<number>([mixedPort, apiPort]);
    for (const row of extra) {
      if (seen.has(row.port)) {
        setError(t("settings.dupPort", { n: row.port }));
        return;
      }
      seen.add(row.port);
    }
    applyingRef.current = true;
    const generationAtStart = applyGenerationRef.current;
    setBusy(true);
    setError(null);
    let succeeded = false;
    try {
      const s = await updateSettings({
        mixedPort,
        allowLan,
        apiPort,
        apiSecretEnabled,
        extraInbounds: extra,
        probeUrl: probe.trim() || null,
        tunStack: tunStack.trim() || "mixed",
        tunIpv6Enabled: tunIpv6,
        blockQuic,
        bypassLan,
      });
      setSettings(s);
      // These options are consumed when sing-box starts; apply them together.
      const status = await getProxyStatus().catch(() => null);
      if (status?.running) {
        await restartProxy();
      }
      succeeded = true;
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    } finally {
      applyingRef.current = false;
      setBusy(false);
      // Re-queue only if either this attempt landed cleanly (so a still-dirty
      // draft is a genuinely new edit) or the user changed something else
      // while it was in flight. A failing attempt whose draft never changes
      // (e.g. sing-box can't bind the LAN listener) must not retry itself
      // forever with no backoff — that is the restart-loop bug this guards.
      if (succeeded || applyGenerationRef.current !== generationAtStart) {
        void autoApplyRef.current();
      }
    }
  }, [allowLan, api, apiSecretEnabled, blockQuic, bypassLan, extra, mixed, probe, settings, t, tunIpv6, tunStack]);

  autoApplyRef.current = autoApplyNetwork;

  // Debounce so typing a port number doesn't restart the core per keystroke;
  // toggles / selects / modal saves settle within the same short window.
  useEffect(() => {
    if (!settings) return;
    applyGenerationRef.current += 1;
    const timer = setTimeout(() => void autoApplyRef.current(), 600);
    return () => clearTimeout(timer);
    // Fire on any draft change; autoApplyNetwork itself decides if there is
    // anything valid and dirty to commit.
  }, [settings, mixed, allowLan, api, apiSecretEnabled, probe, tunStack, tunIpv6, blockQuic, bypassLan, extra]);

  // —— Extra inbound listeners (draft rows + modal editor) ——

  function openAddInbound() {
    setInboundEditId(null);
    setInboundKind("mixed");
    setInboundPort("");
    setInboundLan(false);
    setInboundError(null);
    setInboundOpen(true);
  }

  function openEditInbound(row: ExtraInbound) {
    setInboundEditId(row.id);
    setInboundKind(row.kind);
    setInboundPort(String(row.port));
    setInboundLan(!!row.allow_lan);
    setInboundError(null);
    setInboundOpen(true);
  }

  /** Validate in the modal, then commit to the list (auto-applied + core
   * restart via the debounced effect). */
  function saveInbound() {
    const port = Number(inboundPort);
    if (!Number.isFinite(port) || port < 1 || port > 65535) {
      setInboundError(t("settings.invalidExtraPort"));
      return;
    }
    const others = extra.filter((r) => r.id !== inboundEditId);
    const taken = new Set<number>([
      ...others.map((r) => r.port),
      settings?.mixed_port ?? 0,
      settings?.api_port ?? 0,
    ]);
    if (taken.has(port)) {
      setInboundError(t("settings.dupPort", { n: port }));
      return;
    }
    const entry: ExtraInbound = {
      id: inboundEditId ?? `in-${Math.random().toString(36).slice(2, 10)}`,
      kind: inboundKind,
      port,
      allow_lan: inboundLan,
    };
    setExtra((prev) =>
      inboundEditId == null
        ? [...prev, entry]
        : prev.map((r) => (r.id === inboundEditId ? entry : r)),
    );
    setInboundOpen(false);
  }

  function removeInbound(id: string) {
    setExtra((prev) => prev.filter((r) => r.id !== id));
  }

  async function onCopySecret() {
    const secret = settings?.clash_api_secret;
    if (!secret) return;
    try {
      await navigator.clipboard.writeText(secret);
      setSecretCopied(true);
      window.setTimeout(() => setSecretCopied(false), 1500);
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    }
  }

  /** Copy-feedback for the truncated path lines (kernel rows + app card). */
  const [copiedPath, setCopiedPath] = useState<string | null>(null);
  async function onCopyPath(path: string) {
    try {
      await navigator.clipboard.writeText(path);
      setCopiedPath(path);
      window.setTimeout(() => setCopiedPath(null), 1500);
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    }
  }

  /** Relative time for the app card's "last check" row (backend stores unix
   *  seconds). */
  function formatCheckedAt(ts: number | null | undefined): string {
    if (!ts) return "—";
    const minutes = Math.floor((Date.now() / 1000 - ts) / 60);
    if (minutes < 1) return t("settings.relJustNow");
    if (minutes < 60) return t("settings.relMinAgo", { n: minutes });
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return t("settings.relHourAgo", { n: hours });
    return t("settings.relDayAgo", { n: Math.floor(hours / 24) });
  }

  /** User-triggered secret rotation; backend restarts a running core so the
   * new secret is live immediately. */
  async function onRegenerateSecret() {
    setError(null);
    setBusy(true);
    try {
      const s = await regenerateApiSecret();
      setSettings(s);
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    } finally {
      setBusy(false);
    }
  }

  /** Downloads a core (latest, or an exact tag for factory restore).
   *  Returns whether the install succeeded — callers may follow up with a
   *  restart when the new binary must take effect immediately. */
  async function onDownloadCore(kind: CoreKind, tag?: string | null): Promise<boolean> {
    setCoreError(null);
    const status = await getProxyStatus().catch(() => null);
    const viaProxy = !!status?.running;
    setCoreProxyAvailable(viaProxy);
    beginCoreDownload(kind, viaProxy, tag ?? "");
    try {
      await downloadCore(kind, tag ?? null);
      await reloadCore();
      clearCoreDownload();
      return true;
    } catch (e) {
      const message = typeof e === "string" ? e : String(e);
      setCoreError(message);
      setCoreDownloadError(kind, message);
      return false;
    }
  }

  async function onCheckCoreUpdate(kind: CoreKind) {
    await runCoreUpdateCheck(kind, cores[kind]?.version ?? null, corePrerelease[kind]);
  }

  /** Core card "factory reset": with a bundled copy, drop the user-downloaded
   *  binary so the bundled one takes over (backend restarts a running core of
   *  the same kind). Without one (default installs bundle only sing-box),
   *  restore = re-downloading the pinned factory version through the normal
   *  download pipeline, progress bar included. */
  async function onRestoreCore(kind: CoreKind) {
    const info = cores[kind];
    if (info?.bundled_version) {
      if (!confirm(t("settings.coreRestoreConfirm", { v: info.bundled_version }))) return;
      setCoreError(null);
      beginCoreDownload(kind, false, info.bundled_version);
      try {
        await resetCoreToBundled(kind);
        await reloadCore();
        clearCoreDownload();
      } catch (e) {
        const message = typeof e === "string" ? e : String(e);
        setCoreError(message);
        setCoreDownloadError(kind, message);
      }
      return;
    }
    const factory = info?.factory_version;
    if (!factory) return;
    if (!confirm(t("settings.coreRestoreDlConfirm", { v: factory }))) return;
    const ok = await onDownloadCore(kind, factory);
    // Mirror the bundled path: when this kind is the running active core,
    // restart so the factory binary takes effect immediately (a plain
    // "update core" download deliberately leaves that to the user).
    if (ok && (settings?.core_type ?? "singbox") === kind) {
      const status = await getProxyStatus().catch(() => null);
      if (status?.running) {
        await restartProxy();
      }
    }
  }

  /** Switch the active core; a running core restarts onto the new binary. */
  async function onSwitchCore(kind: CoreKind) {
    if (settings?.core_type === kind) return;
    setCoreError(null);
    try {
      const s = await setCoreType(kind);
      setSettings(s);
    } catch (e) {
      setCoreError(typeof e === "string" ? e : String(e));
    }
  }

  /** One compact version row per core (sing-box / Xray / mihomo): identity
   *  line, bundled/latest meta + actions, then a quiet path foot line. */
  function renderCoreRow(kind: CoreKind) {
    const info = cores[kind];
    const busy = coreBusyKind === kind;
    // Downloads/restores are globally exclusive (single in-flight backend
    // task, single global store) — disable every core row's actions while
    // ANY kind is busy, not just this row's own.
    const anyBusy = coreBusyKind != null;
    const checking = coreCheckingKind === kind;
    const active = (settings?.core_type ?? "singbox") === kind;
    // Factory-reset target: the bundled copy when the installer ships one,
    // otherwise the pinned factory version (re-downloaded on restore).
    const restoreTarget = info?.bundled_version ?? info?.factory_version ?? null;
    return (
      <div
        className={`card kernel-card${active ? " core-active" : ""}`}
        key={kind}
        title={
          kind === "xray"
            ? t("settings.coreHintXray")
            : kind === "mihomo"
              ? t("settings.coreHintMihomo")
              : t("settings.coreHint")
        }
      >
        <div className="kernel-card-head">
          {/* Monogram tile: cube = sing-box, bolt = Xray, cat head = mihomo. */}
          <div className="ver-mark kernel-mark" aria-hidden>
            <CoreMark kind={kind} />
          </div>
          <span className="kernel-name">
            {info?.name ?? (kind === "xray" ? "Xray" : kind === "mihomo" ? "mihomo" : "sing-box")}
          </span>
          {info?.installed ? (
            !active && (
              <span className={`pill ${info.source === "bundled" ? "ok" : ""}`}>
                {info.source === "bundled"
                  ? t("settings.coreBundled")
                  : t("settings.coreInstalled")}
              </span>
            )
          ) : (
            <span className="pill warn">{t("settings.coreMissing")}</span>
          )}
          {/* Radio-style enable: clicking switches the active core (a
             running core restarts onto the new binary). */}
          <button
            type="button"
            className={`core-radio${active ? " on" : ""}`}
            aria-pressed={active}
            aria-label={t("settings.coreUse")}
            title={t("settings.coreUse")}
            disabled={coreBusyKind != null}
            onClick={() => void onSwitchCore(kind)}
          >
            <span className="core-radio-dot" aria-hidden />
          </button>
        </div>

        <div className="kernel-card-platform muted mono">
          {info?.platform ?? "…"}
        </div>

        <div className="kernel-card-ver">
          <span className="stat-label">{t("settings.coreCurrent")}</span>
          <span className="kernel-version mono">
            {info?.version ?? "—"}
            {info?.source === "downloaded" ? (
              <span className="pill">{t("settings.coreUser")}</span>
            ) : null}
          </span>
        </div>

        <div className="kernel-card-meta">
          <span className="mono">
            {t("settings.coreBundledShort")} {info?.bundled_version ?? "—"}
          </span>
          <span className="mono">
            {t("settings.coreLatestShort")} {info?.latest_version ?? "—"}
          </span>
          {info?.update_available ? (
            <span className="pill warn">{t("settings.coreUpdateAvail")}</span>
          ) : null}
        </div>

        <div className="kernel-row-actions">
          {/* Hidden for now — an Xray pre-release picked up via this toggle
             broke connectivity (protocol translation issue suspected).
             Keeping the toggle/state/backend path in place so it can come
             back once that's root-caused; just not user-reachable. */}
          {false && (
            <span className="core-prerelease-toggle mono muted" title={t("settings.corePrereleaseHint")}>
              {t("settings.corePrerelease")}
              <GlassSwitchControl
                checked={corePrerelease[kind]}
                title={t("settings.corePrereleaseHint")}
                disabled={anyBusy || checking}
                size="sm"
                onChange={(next) => setCorePrerelease((prev) => ({ ...prev, [kind]: next }))}
              />
            </span>
          )}
          <GlassButton
            icon="↻"
            disabled={anyBusy || checking || !info}
            onClick={() => void onCheckCoreUpdate(kind)}
          >
            {checking
              ? t("settings.coreChecking")
              : t("settings.coreCheck")}
          </GlassButton>
          {/* One stable label in every state — the previous state-dependent
             wording (下载/更新内核/重新下载) flip-flopped with the
             staged/downloaded source and read like random renames. Update
             availability is already signaled by the pill in the meta row.
             Pins to the checked latest_version (once a check has run) so a
             pre-release picked up via the toggle above is what actually
             gets installed — otherwise the backend would look up latest
             itself and silently land back on the non-prerelease tag. */}
          <GlassButton
            icon="⤓"
            disabled={anyBusy || checking}
            onClick={() => void onDownloadCore(kind, info?.latest_version ?? null)}
          >
            {busy ? t("settings.coreDownloading") : t("settings.coreDownload")}
          </GlassButton>
          {/* Factory reset: always available on an installed core — the
             staged-vs-downloaded `source` flips whenever the kernel restarts
             (resolve re-stages the bundled binary into bin/), so keying
             visibility off it made the button appear/vanish unpredictably.
             Re-clicking at the target version is a harmless re-stage /
             factory re-download. Only a missing core has nothing to reset. */}
          {info?.installed && restoreTarget ? (
            <GlassButton
              icon="⟲"
              disabled={anyBusy || checking}
              title={
                info.bundled_version
                  ? t("settings.coreRestoreHint", { v: info.bundled_version })
                  : t("settings.coreRestoreDlHint", { v: restoreTarget })
              }
              onClick={() => void onRestoreCore(kind)}
            >
              {t("settings.coreRestore")}
            </GlassButton>
          ) : null}
        </div>

        <div className="kernel-row-foot">
          {active && coreProxyAvailable && (
            <span className="kernel-run">
              <span className="kernel-run-dot" aria-hidden />
              {t("dashboard.coreRunning")}
            </span>
          )}
          {info?.path && (
            <>
              <code className="kernel-path mono" title={info.path}>
                {info.path}
              </code>
              <GlassButton
                iconOnly
                icon={copiedPath === info.path ? "✓" : "⧉"}
                className="kernel-copy-btn"
                title={copiedPath === info.path ? t("common.copied") : t("common.copy")}
                aria-label={t("common.copy")}
                onClick={() => void onCopyPath(info.path!)}
              />
            </>
          )}
        </div>
      </div>
    );
  }

  async function patchApp(partial: Parameters<typeof updateSettings>[0]) {
    setError(null);
    try {
      const s = await updateSettings(partial);
      setSettings(s);
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
      try {
        const s = await getSettings();
        setSettings(s);
      } catch {
        /* ignore */
      }
    }
  }

  /** Protocols a sidecar core can carry, with per-row target cores from the
   *  Rust support surface: every listed protocol is mihomo-capable (negative
   *  list), and all but masque are also Xray-capable — so e.g. hysteria2 can
   *  egress through either sidecar. masque is mihomo-only (sing-box/Xray
   *  both lack a masque outbound): its "auto" state has no native fallback,
   *  the nodes are simply filtered, so the option reads 未启用 instead of
   *  follow-main. WireGuard stays Xray-labeled to match the existing plan
   *  behavior (endpoint-shaped, excluded from delegation in
   *  `compute_sidecar_plan`). Nodes whose exact transport combo the target
   *  core rejects (e.g. REALITY+ws on Xray) fall back to native sing-box
   *  outbounds at build time. */
  const MULTICORE_PROTOCOLS: {
    value: string;
    label: string;
    cores: string[];
    autoDisabled?: boolean;
  }[] = [
    { value: "vmess", label: "VMess", cores: ["xray", "mihomo"] },
    { value: "vless", label: "VLESS", cores: ["xray", "mihomo"] },
    { value: "shadowsocks", label: "Shadowsocks", cores: ["xray", "mihomo"] },
    { value: "trojan", label: "Trojan", cores: ["xray", "mihomo"] },
    { value: "hysteria2", label: "Hysteria2", cores: ["xray", "mihomo"] },
    { value: "socks5", label: "SOCKS5", cores: ["xray", "mihomo"] },
    { value: "http", label: "HTTP", cores: ["xray", "mihomo"] },
    { value: "wireguard", label: "WireGuard", cores: ["xray"] },
    {
      value: "masque",
      label: "MASQUE",
      cores: ["mihomo"],
      autoDisabled: true,
    },
  ];
  /** protocol → pinned sidecar core ("auto" = follow the main core). */
  const pinnedCores = new Map(
    (settings?.protocol_cores ?? []).map((e) => [e.protocol, e.core]),
  );
  /** Multi-core only exists under the sing-box main core; switching cores
   *  auto-disables it (backend mirrors this in set_core_type). */
  const multiCoreAvailable = (settings?.core_type ?? "singbox") === "singbox";

  /** Row change in the multi-core table: "auto" removes the delegation
   *  (protocol follows the main core), anything else pins it. */
  function onProtocolCoreChange(protocol: string, core: string) {
    const rest = (settings?.protocol_cores ?? []).filter(
      (e) => e.protocol !== protocol,
    );
    const next =
      core === "auto" ? rest : [...rest, { protocol, core }];
    void patchApp({ protocolCores: next });
  }

  async function onCommitSidecarPort() {
    const parsed = Number.parseInt(sidecarPort, 10);
    const current = settings?.sidecar_port ?? 20890;
    if (
      !Number.isFinite(parsed) ||
      parsed <= 0 ||
      parsed > 65535 ||
      parsed === current
    ) {
      setSidecarPort(String(current));
      return;
    }
    await patchApp({ sidecarPort: parsed });
  }

  async function onChangeLocale(next: Locale) {
    if (next === locale) return;
    setError(null);
    try {
      await setLocale(next);
      const s = await getSettings();
      setSettings(s);
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    }
  }

  async function onChangeTheme(next: ThemeId) {
    if (next === theme) return;
    setError(null);
    try {
      await setTheme(next);
      const s = await getSettings();
      setSettings(s);
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    }
  }

  const customRuntime = (settings?.runtime_source ?? "").startsWith("singbox:");
  /** Xray has no Clash API / clash_api_secret concept — hide that control. */
  const xrayCore = (settings?.core_type ?? "singbox") === "xray";

  useEffect(() => {
    if (customRuntime && CUSTOM_BLOCKED_TABS.has(tab)) {
      setTab("app");
    }
  }, [customRuntime, tab]);

  const visibleTab =
    customRuntime && CUSTOM_BLOCKED_TABS.has(tab) ? "app" : tab;

  // Reload PAC service state + list whenever the tab becomes visible.
  useEffect(() => {
    if (visibleTab !== "pac") return;
    void reloadPacTab();
    // Mirror presets are static; fetch once for the source picker.
    listPacSourcePresets()
      .then(setPacPresets)
      .catch(() => setPacPresets([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visibleTab]);

  // Seed the PAC port draft from the loaded status.
  useEffect(() => {
    if (pacStatus) setPacPort(String(pacStatus.port));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pacStatus?.port]);

  // Auto-updated in the background → refresh the panel without a manual reload.
  useEffect(() => {
    const unlisten = listen("pac-list-updated", () => {
      void reloadPacTab();
    });
    return () => {
      void unlisten.then((off) => off());
    };
  }, [reloadPacTab]);

  // Interval options for the auto-update select.
  const pacIntervalOptions = useMemo(
    () =>
      ([1, 6, 12, 24, 72] as const).map((hours) => ({
        value: String(hours),
        label:
          hours === 1
            ? t("pac.interval1h")
            : hours === 6
              ? t("pac.interval6h")
              : hours === 12
                ? t("pac.interval12h")
                : hours === 24
                  ? t("pac.interval24h")
                  : t("pac.interval72h"),
      })),
    [t],
  );

  // ←/→ cycle through the settings sub-tabs, skipping any the custom
  // runtime blocks. Ignored while typing (input/textarea/select) so text
  // cursor movement and dropdown navigation are unaffected.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
      const active = document.activeElement;
      const tag = active?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      if ((active as HTMLElement | null)?.isContentEditable) return;

      const enabled = tabs.filter(
        (x) => !(customRuntime && CUSTOM_BLOCKED_TABS.has(x.id)),
      );
      const idx = enabled.findIndex((x) => x.id === visibleTab);
      if (idx === -1) return;
      e.preventDefault();
      const delta = e.key === "ArrowRight" ? 1 : -1;
      const next = enabled[(idx + delta + enabled.length) % enabled.length];
      setTab(next.id);
      setError(null);
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [tabs, customRuntime, visibleTab]);

  const needsSettings =
    visibleTab === "app" || visibleTab === "ports" || visibleTab === "core";
  if (needsSettings && !settings && !error) {
    return <div className="page empty">{t("common.loading")}</div>;
  }

  const activeTab = tabs.find((x) => x.id === visibleTab)!;

  return (
    <div className="page settings-page settings-wide">
      <header className="page-header">
        <div>
          <h1>{t("settings.title")}</h1>
          <p className="page-desc">{activeTab.hint}</p>
        </div>
      </header>

      {/* Corner links live in the version tab's app card (project home lives
       * next to the app's own version info). */}

      <GlassSeg
        value={visibleTab}
        ariaLabel="Settings sections"
        onChange={(v) => {
          if (customRuntime && CUSTOM_BLOCKED_TABS.has(v)) return;
          setTab(v as SettingsTab);
          setError(null);
        }}
        disabledValues={customRuntime ? CUSTOM_BLOCKED_TABS : undefined}
        titles={
          customRuntime
            ? {
                rules: t("config.customDisabled"),
                chain: t("config.customDisabled"),
                multiCore: t("config.customDisabled"),
                dns: t("config.customDisabled"),
                hosts: t("config.customDisabled"),
              }
            : undefined
        }
        options={tabs.map((x) => ({ value: x.id, label: x.label }))}
      />

      {error &&
        visibleTab !== "rules" &&
        visibleTab !== "chain" &&
        visibleTab !== "multiCore" &&
        visibleTab !== "dns" &&
        visibleTab !== "hosts" && (
        <ErrorModal message={error} onClose={() => setError(null)} />
      )}

      {/* key={tab} remounts on tab switch → triggers the page-enter fade/slide. */}
      <div
        className={`page-enter${
          visibleTab === "app"
            ? " settings-app-page"
            : visibleTab === "ports"
              ? " settings-ports-page"
              : ""
        }${
          visibleTab === "rules" ||
          visibleTab === "chain" ||
          visibleTab === "multiCore" ||
          visibleTab === "dns"
            ? " settings-scroll-embed"
            : ""
        }`}
        key={visibleTab}
      >
        {!customRuntime && visibleTab === "rules" && <RulesPage embedded />}

        {!customRuntime && visibleTab === "chain" && <ChainPage embedded />}

        {visibleTab === "multiCore" && settings && (
          <section className="settings-panel" aria-label="Core settings">
            <div className="card sidecar-card">
              <div className="via-proxy-row">
                <div>
                  <div className="sys-proxy-title">{t("settings.tlsFragment")}</div>
                  <div className="sys-proxy-desc">{t("settings.tlsFragmentDesc")}</div>
                </div>
              </div>
              <div className="sidecar-body">
                <div className="multicore-grid">
                  <div className="multicore-row">
                    <code>sing-box</code>
                    <GlassSwitchControl
                      checked={!!settings.tls_fragment_singbox}
                      title={t("settings.tlsFragmentSingbox")}
                      disabled={customRuntime}
                      onChange={(v) => void patchApp({ tlsFragmentSingbox: v })}
                    />
                  </div>
                  <div className="multicore-row">
                    <code>Xray</code>
                    <GlassSwitchControl
                      checked={!!settings.tls_fragment_xray}
                      title={t("settings.tlsFragmentXray")}
                      disabled={customRuntime}
                      onChange={(v) => void patchApp({ tlsFragmentXray: v })}
                    />
                  </div>
                </div>
                <div className="field-hint muted">
                  {t("settings.tlsFragmentHint")}
                </div>
              </div>
            </div>

            <div className="card sidecar-card">
              <div className="via-proxy-row">
                <div>
                  <div className="sys-proxy-title">
                    {t("settings.multiCore")}
                    {settings.multi_core_enabled && (
                      <span
                        className={`pill sidecar-pill${sidecarRunning ? " ok" : ""}`}
                      >
                        {sidecarRunning
                          ? t("settings.multiCoreRunning")
                          : t("settings.multiCoreIdle")}
                      </span>
                    )}
                  </div>
                  <div className="sys-proxy-desc">
                    {t("settings.multiCoreDesc")}
                  </div>
                </div>
                <GlassSwitchControl
                  checked={!!settings.multi_core_enabled}
                  title={t("settings.multiCore")}
                  disabled={customRuntime || !multiCoreAvailable}
                  onChange={(v) => void patchApp({ multiCoreEnabled: v })}
                />
              </div>

              {!multiCoreAvailable && (
                <div className="field-hint muted">
                  {t("settings.multiCoreSingboxOnly")}
                </div>
              )}

              {settings.multi_core_enabled && (
                <div className="sidecar-body">
                  <div className="multicore-grid">
                    {MULTICORE_PROTOCOLS.map((p) => {
                      const pinned = pinnedCores.get(p.value);
                      const pinInTargets =
                        pinned !== undefined && p.cores.includes(pinned);
                      const currentValue =
                        pinInTargets && pinned ? pinned : "auto";
                      return (
                        <div className="multicore-row" key={p.value}>
                          <code>{p.label}</code>
                          <SolidSelect
                            value={currentValue}
                            aria-label={p.label}
                            disabled={customRuntime}
                            onChange={(v) =>
                              onProtocolCoreChange(p.value, v)
                            }
                            options={[
                              {
                                value: "auto",
                                label: p.autoDisabled
                                  ? t("settings.multiCoreMasqueDisabled")
                                  : t("settings.multiCoreFollowMain"),
                              },
                              ...p.cores.map((c) =>
                                c === "xray"
                                  ? { value: c, label: "Xray" }
                                  : { value: c, label: "mihomo" },
                              ),
                            ]}
                          />
                        </div>
                      );
                    })}
                  </div>
                  <div className="field-hint muted">
                    {t("settings.multiCoreTableHint")}
                  </div>
                  {pinnedCores.size === 0 && (
                    <div className="field-hint sidecar-warn">
                      {t("settings.multiCoreNoProtocols")}
                    </div>
                  )}
                  <label className="field field-inline">
                    <span className="field-inline-row">
                      <span className="field-inline-label">
                        {t("settings.multiCorePort")}
                      </span>
                      <input
                        autoCapitalize="off"
                        autoCorrect="off"
                        spellCheck={false}
                        inputMode="numeric"
                        className="mono"
                        value={sidecarPort}
                        onChange={(e) => setSidecarPort(e.target.value)}
                        onBlur={() => void onCommitSidecarPort()}
                        onKeyDown={(e) => {
                          if (e.key === "Enter")
                            (e.target as HTMLInputElement).blur();
                        }}
                      />
                    </span>
                    <span className="field-hint muted">
                      {t("settings.multiCorePortHint")}
                    </span>
                  </label>
                </div>
              )}
            </div>
          </section>
        )}
        {!customRuntime && visibleTab === "dns" && (
          <DnsPage embedded />
        )}
        {!customRuntime && visibleTab === "hosts" && <HostsPage embedded />}
      {visibleTab === "app" && settings && (
        <section className="settings-panel" aria-label="Application">
          <div className="card settings-app-card">
            <div className="settings-app-cols">
              <div className="settings-app-col">
              <div className="settings-app-row settings-app-pref">
                <div className="settings-app-text">
                  <div className="settings-app-title">{t("settings.language")}</div>
                  <div className="settings-app-desc muted">
                    {t("settings.languageDesc")}
                  </div>
                </div>
                <GlassSeg
                  value={locale}
                  ariaLabel={t("settings.language")}
                  disabled={busy}
                  onChange={(v) => void onChangeLocale(v as Locale)}
                  options={[
                    { value: "zh", label: t("settings.langZh") },
                    { value: "en", label: t("settings.langEn") },
                  ]}
                />
              </div>
              <AppToggle
                title={t("settings.launchAtLogin")}
                desc={t("settings.launchAtLoginDesc")}
                checked={!!settings?.launch_at_login}
                disabled={busy}
                onChange={(v) => void patchApp({ launchAtLogin: v })}
              />
              <AppToggle
                title={t("settings.silentStart")}
                desc={t("settings.silentStartDesc")}
                checked={!!settings?.silent_start}
                disabled={busy}
                onChange={(v) => void patchApp({ silentStart: v })}
              />
              <AppToggle
                title={t("settings.autoStartProxy")}
                desc={t("settings.autoStartProxyDesc")}
                checked={!!settings?.auto_start_proxy}
                disabled={busy}
                onChange={(v) => void patchApp({ autoStartProxy: v })}
              />
              <AppToggle
                title={t("settings.closeToTray")}
                desc={t("settings.closeToTrayDesc")}
                checked={settings?.close_to_tray !== false}
                disabled={busy}
                onChange={(v) => void patchApp({ closeToTray: v })}
              />
              <AppToggle
                title={t("settings.unloadUi")}
                desc={t("settings.unloadUiDesc")}
                checked={!!settings?.unload_ui_on_tray}
                disabled={busy}
                onChange={(v) => void patchApp({ unloadUiOnTray: v })}
              />
              <AppToggle
                title={t("settings.closeOnSwitch")}
                desc={t("settings.closeOnSwitchDesc")}
                checked={!!settings?.close_connections_on_switch}
                disabled={busy || (settings?.runtime_source ?? "").startsWith("singbox:")}
                onChange={(v) => void patchApp({ closeConnectionsOnSwitch: v })}
              />
              <AppToggle
                title={t("settings.findProcess")}
                desc={t("settings.findProcessDesc")}
                checked={settings?.find_process !== false}
                disabled={busy || (settings?.runtime_source ?? "").startsWith("singbox:")}
                onChange={(v) => void patchApp({ findProcess: v })}
              />
              </div>
              <div className="settings-app-col">
              <div className="settings-app-row settings-app-pref">
                <div className="settings-app-text">
                  <div className="settings-app-title">{t("settings.theme")}</div>
                  <div className="settings-app-desc muted">
                    {t("settings.themeDesc")}
                  </div>
                </div>
                <GlassSeg
                  value={theme}
                  ariaLabel={t("settings.theme")}
                  disabled={busy}
                  onChange={(v) => void onChangeTheme(v as ThemeId)}
                  options={[
                    { value: "aerospace", label: t("settings.themeAerospace") },
                    { value: "day", label: t("settings.themeDay") },
                  ]}
                />
              </div>
              <div className="settings-app-row settings-app-pref settings-hero-row settings-duo-col">
                <div className="settings-app-text">
                  <div className="settings-app-title">{t("settings.glassFrost")}</div>
                  <div className="settings-app-desc muted">
                    {t("settings.glassFrostDesc")}
                  </div>
                </div>
                <GlassSeg
                  value={glassFrost ? "frost" : "lite"}
                  ariaLabel={t("settings.glassFrost")}
                  disabled={busy}
                  onChange={(v) => void setGlassFrost(v === "frost")}
                  options={[
                    { value: "lite", label: t("settings.glassFrostLite") },
                    { value: "frost", label: t("settings.glassFrostFull") },
                  ]}
                />
              </div>
              <div className="settings-app-row settings-app-pref settings-accent-row">
                <div className="settings-app-text">
                  <div className="settings-app-title">{t("settings.accent")}</div>
                  <div className="settings-app-desc muted">
                    {t("settings.accentDesc")}
                  </div>
                </div>
                <div
                  className="settings-accent-swatches"
                  role="group"
                  aria-label={t("settings.accent")}
                >
                  {ACCENTS.map((a) => (
                    <button
                      key={a.id}
                      type="button"
                      className={`settings-accent-dot ${accent === a.id ? "active" : ""}`}
                      style={{ background: a[theme], color: a[theme] }}
                      title={t(ACCENT_LABEL_KEY[a.id] ?? "settings.accent")}
                      aria-label={t(ACCENT_LABEL_KEY[a.id] ?? "settings.accent")}
                      aria-pressed={accent === a.id}
                      disabled={busy}
                      onClick={() => void setAccent(a.id)}
                    >
                      {accent === a.id ? (
                        <span className="settings-accent-check">✓</span>
                      ) : (
                        ""
                      )}
                    </button>
                  ))}
                  <button
                    type="button"
                    className={`settings-accent-dot ${isCustomHexAccent(accent) ? "active" : ""}`}
                    style={
                      isCustomHexAccent(accent)
                        ? { background: accent, color: accent }
                        : { background: CUSTOM_DOT_RAINBOW }
                    }
                    title={t("accent.custom")}
                    aria-label={t("accent.custom")}
                    aria-pressed={isCustomHexAccent(accent)}
                    disabled={busy}
                    onClick={() => setAccentPickerOpen(true)}
                  >
                    {isCustomHexAccent(accent) ? (
                      <span className="settings-accent-check">✓</span>
                    ) : (
                      ""
                    )}
                  </button>
                </div>
              </div>
              <div className="settings-app-row settings-app-pref settings-accent-row">
                <div className="settings-app-text">
                  <div className="settings-app-title">{t("settings.glow")}</div>
                  <div className="settings-app-desc muted">
                    {t("settings.glowDesc")}
                  </div>
                </div>
                <div
                  className="settings-accent-swatches"
                  role="group"
                  aria-label={t("settings.glow")}
                >
                  {/* "Follow accent" mirrors the accent's effective shade. */}
                  <button
                    type="button"
                    className={`settings-accent-dot ${glow === "accent" ? "active" : ""}`}
                    style={{
                      background: resolveAccent(accent)[theme],
                      color: resolveAccent(accent)[theme],
                    }}
                    title={t("settings.glowFollow")}
                    aria-label={t("settings.glowFollow")}
                    aria-pressed={glow === "accent"}
                    disabled={busy}
                    onClick={() => void setGlow("accent")}
                  >
                    {glow === "accent" ? (
                      <span className="settings-accent-check">✓</span>
                    ) : (
                      ""
                    )}
                  </button>
                  {ACCENTS.map((a) => (
                    <button
                      key={a.id}
                      type="button"
                      className={`settings-accent-dot ${glow === a.id ? "active" : ""}`}
                      style={{ background: a[theme], color: a[theme] }}
                      title={t(ACCENT_LABEL_KEY[a.id] ?? "settings.glow")}
                      aria-label={t(ACCENT_LABEL_KEY[a.id] ?? "settings.glow")}
                      aria-pressed={glow === a.id}
                      disabled={busy}
                      onClick={() => void setGlow(a.id)}
                    >
                      {glow === a.id ? (
                        <span className="settings-accent-check">✓</span>
                      ) : (
                        ""
                      )}
                    </button>
                  ))}
                  <button
                    type="button"
                    className={`settings-accent-dot ${isCustomHexAccent(glow) ? "active" : ""}`}
                    style={
                      isCustomHexAccent(glow)
                        ? { background: glow, color: glow }
                        : { background: CUSTOM_DOT_RAINBOW }
                    }
                    title={t("accent.custom")}
                    aria-label={t("accent.custom")}
                    aria-pressed={isCustomHexAccent(glow)}
                    disabled={busy}
                    onClick={() => setGlowPickerOpen(true)}
                  >
                    {isCustomHexAccent(glow) ? (
                      <span className="settings-accent-check">✓</span>
                    ) : (
                      ""
                    )}
                  </button>
                </div>
              </div>
              <div className="settings-app-row settings-app-pref settings-hero-row">
                <div className="settings-app-text">
                  <div className="settings-app-title">{t("settings.heroStyle")}</div>
                  <div className="settings-app-desc muted">
                    {t("settings.heroStyleDesc")}
                  </div>
                </div>
                <GlassSeg
                  value={heroStyle}
                  ariaLabel={t("settings.heroStyle")}
                  disabled={busy}
                  onChange={(v) => void setHeroStyle(v as HeroStyle)}
                  options={[
                    { value: "particle", label: t("settings.heroStyleParticle") },
                    { value: "classic", label: t("settings.heroStyleClassic") },
                    { value: "smiley", label: t("settings.heroStyleSmiley") },
                  ]}
                />
              </div>
              <div className="settings-app-row settings-app-pref settings-tray-icon-row settings-duo-col">
                <div className="settings-app-text">
                  <div className="settings-app-title">{t("settings.trayIcon")}</div>
                </div>
                <TrayIconPicker
                  value={settings?.tray_icon}
                  disabled={busy}
                  aria-label={t("settings.trayIcon")}
                  onChange={(v) => void patchApp({ trayIcon: v })}
                />
              </div>
            </div>
            </div>
          </div>
          <p className="settings-panel-note muted">{t("settings.toggleSaveNote")}</p>
        </section>
      )}

      {visibleTab === "ports" && settings && (
        <section className="settings-panel" aria-label="Ports">
          <div className="settings-ports-columns">
            <div className="card settings-form settings-form-grid">
              <label className="field field-inline field-span-2">
                <span className="field-inline-row">
                  <span className="field-inline-label">{t("settings.mixedPort")}</span>
                  <input
                    autoCapitalize="off"
                    autoCorrect="off"
                    spellCheck={false}
                    value={mixed}
                    disabled={(settings?.runtime_source ?? "").startsWith("singbox:")}
                    onChange={(e) => setMixed(e.target.value)}
                  />
                </span>
                <span className="field-hint muted">{t("settings.mixedPortHint")}</span>
              </label>
              <div className="via-proxy-row field-span-2">
                <div>
                  <div className="sys-proxy-title">{t("settings.allowLan")}</div>
                  <div className="sys-proxy-desc">
                    {t("settings.allowLanDesc")}
                  </div>
                </div>
                <GlassSwitchControl
                  checked={allowLan}
                  title={t("settings.allowLan")}
                  disabled={busy || (settings?.runtime_source ?? "").startsWith("singbox:")}
                  onChange={setAllowLan}
                />
              </div>
              <label className="field field-span-2">
                <span>{t("settings.probeUrl")}</span>
                <input
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  value={probe}
                  onChange={(e) => setProbe(e.target.value)}
                  placeholder="https://…"
                />
              </label>
              <div className="field field-span-2">
                <span>{t("settings.tunStack")}</span>
                <SolidSelect
                  value={tunStack}
                  onChange={setTunStack}
                  aria-label={t("settings.tunStack")}
                  options={[
                    { value: "mixed", label: "mixed" },
                    { value: "system", label: "system" },
                    { value: "gvisor", label: "gvisor" },
                  ]}
                />
                <span className="field-hint muted">
                  {t("settings.tunStackHint")}{" "}
                  <span className="mono">
                    {settings?.tun_enabled
                      ? t("common.enabled")
                      : t("common.disabled")}
                  </span>
                </span>
              </div>
              <div className="via-proxy-row field-span-2">
                <div>
                  <div className="sys-proxy-title">{t("settings.tunIpv6")}</div>
                  <div className="sys-proxy-desc">{t("settings.tunIpv6Desc")}</div>
                </div>
                <GlassSwitchControl
                  checked={tunIpv6}
                  title={t("settings.tunIpv6")}
                  disabled={busy}
                  onChange={setTunIpv6}
                />
              </div>
              <div className="via-proxy-row field-span-2">
                <div>
                  <div className="sys-proxy-title">{t("settings.blockQuic")}</div>
                  <div className="sys-proxy-desc">{t("settings.blockQuicDesc")}</div>
                </div>
                <GlassSwitchControl
                  checked={blockQuic}
                  title={t("settings.blockQuic")}
                  disabled={busy}
                  onChange={setBlockQuic}
                />
              </div>
              <div className="via-proxy-row field-span-2">
                <div>
                  <div className="sys-proxy-title">{t("settings.bypassLan")}</div>
                  <div className="sys-proxy-desc">{t("settings.bypassLanDesc")}</div>
                </div>
                <GlassSwitchControl
                  checked={bypassLan}
                  title={t("settings.bypassLan")}
                  disabled={busy}
                  onChange={setBypassLan}
                />
              </div>
              <div className="field-divider field-span-2" />
              <label className="field field-inline field-span-2">
                <span className="field-inline-row">
                  <span className="field-inline-label">
                    {t("settings.apiPort")}
                    <span className="field-badge field-badge-warn">
                      {t("settings.apiPortBadge")}
                    </span>
                  </span>
                  <input
                    autoCapitalize="off"
                    autoCorrect="off"
                    spellCheck={false}
                    value={api}
                    disabled={(settings?.runtime_source ?? "").startsWith("singbox:")}
                    onChange={(e) => setApi(e.target.value)}
                  />
                </span>
                <span className="field-hint field-hint-warn">
                  {t("settings.apiPortHint")}
                </span>
              </label>
              <div className="via-proxy-row field-span-2">
                <div>
                  <div className="sys-proxy-title">{t("settings.apiSecretEnabled")}</div>
                  <div className="sys-proxy-desc">
                    {t("settings.apiSecretEnabledDesc")}
                  </div>
                </div>
                <GlassSwitchControl
                  checked={apiSecretEnabled}
                  title={t("settings.apiSecretEnabled")}
                  disabled={busy || customRuntime || xrayCore}
                  onChange={setApiSecretEnabled}
                />
              </div>
              {!xrayCore && apiSecretEnabled && (
                <div className="field field-span-2">
                  <span>{t("settings.apiSecret")}</span>
                  <div className="api-secret-row">
                    <input
                      readOnly
                      autoCapitalize="off"
                      autoCorrect="off"
                      spellCheck={false}
                      className="mono api-secret-input"
                      value={settings?.clash_api_secret ?? ""}
                      placeholder={t("settings.apiSecretNone")}
                    />
                    <GlassButton
                      icon={secretCopied ? "✓" : "⧉"}
                      disabled={!settings?.clash_api_secret}
                      onClick={() => void onCopySecret()}
                      title={t("common.copy")}
                    >
                      {secretCopied ? t("common.copied") : t("common.copy")}
                    </GlassButton>
                    <GlassButton
                      icon="↻"
                      disabled={busy || customRuntime}
                      onClick={() => void onRegenerateSecret()}
                      title={t("settings.regenerateSecret")}
                    >
                      {t("settings.regenerateSecret")}
                    </GlassButton>
                  </div>
                  <span className="field-hint muted">
                    {t("settings.apiSecretHint")}
                  </span>
                </div>
              )}
              {netDiagnostics.length > 0 && (
                <div className="field-span-2 diagnostic-banner-list">
                  {netDiagnostics.map((d) => (
                    <div className="diagnostic-banner" key={d.id}>
                      <div className="diagnostic-banner-issue">{d.issue}</div>
                      <div className="diagnostic-banner-suggestion">
                        {d.suggestion}
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
            <div className="card settings-form settings-inbounds-card">
              <div className="settings-network-card-head">
                <div>
                  <strong>{t("settings.extraInbounds")}</strong>
                  <div className="muted">{t("settings.extraInboundsDesc")}</div>
                </div>
                <GlassButton
                  icon="+"
                  disabled={busy || customRuntime || extra.length >= 10}
                  onClick={openAddInbound}
                >
                  {t("settings.addInboundPort")}
                </GlassButton>
              </div>
              <div className="table-wrap inbound-table-wrap">
                <table className="inbound-table">
                  <colgroup>
                    <col style={{ width: 100 }} />
                    <col />
                    <col style={{ width: 60 }} />
                  </colgroup>
                  <thead>
                    <tr>
                      <th>{t("settings.inboundType")}</th>
                      <th>{t("settings.inboundAddr")}</th>
                      <th></th>
                    </tr>
                  </thead>
                  <tbody>
                    {extra.length === 0 ? (
                      <tr>
                        <td colSpan={3} className="muted inbound-empty">
                          {t("settings.extraInboundsEmpty")}
                        </td>
                      </tr>
                    ) : (
                      extra.map((row) => (
                        <tr key={row.id}>
                          <td>
                            <code>{row.kind}</code>
                          </td>
                          <td className="mono">
                            {row.allow_lan ? "0.0.0.0" : "127.0.0.1"}:{row.port}
                          </td>
                          <td>
                            <div className="rule-menu" data-inbound-menu>
                              <button
                                type="button"
                                className="rule-menu-trigger"
                                aria-label={t("common.edit")}
                                aria-haspopup="menu"
                                aria-expanded={menuInboundId === row.id}
                                disabled={busy}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  setMenuInboundId((id) =>
                                    id === row.id ? null : row.id,
                                  );
                                }}
                              >
                                ⋮
                              </button>
                              {menuInboundId === row.id && (
                                <div className="rule-menu-pop" role="menu">
                                  <button
                                    type="button"
                                    role="menuitem"
                                    className="rule-menu-item"
                                    onClick={() => {
                                      setMenuInboundId(null);
                                      openEditInbound(row);
                                    }}
                                  >
                                    {t("common.edit")}
                                  </button>
                                  <button
                                    type="button"
                                    role="menuitem"
                                    className="rule-menu-item danger"
                                    onClick={() => {
                                      setMenuInboundId(null);
                                      removeInbound(row.id);
                                    }}
                                  >
                                    {t("common.delete")}
                                  </button>
                                </div>
                              )}
                            </div>
                          </td>
                        </tr>
                      ))
                    )}
                  </tbody>
                </table>
              </div>
            </div>
          </div>
        </section>
      )}

      {visibleTab === "pac" && (
        <section className="settings-panel" aria-label="PAC list">
          {pacError && (
            <ErrorModal message={pacError} onClose={() => setPacError(null)} />
          )}

          {!pacStatus && !pacList ? (
            <div className="field-hint muted">{t("common.loading")}</div>
          ) : (
            <>
              {/* ---- Rules-page skeleton: left list-actions + groups,
                     right current-group detail. No top config bar. ---- */}
              <div className="rules-layout">
                <aside className="card ruleset-list rules-route-list">
                  <div className="ruleset-list-actions">
                    <GlassButton
                      icon="+"
                      onClick={() => {
                        setPacEditGroup(null);
                        setPacEditName("");
                        setPacEditRemoteUrl("");
                        setPacEditAutoUpdate(false);
                        setPacEditInterval(24);
                        setPacEditOpen(true);
                      }}
                      title={t("pac.newGroupTitle")}
                    >
                      {t("pac.newGroup")}
                    </GlassButton>
                    <GlassButton
                      icon="↺"
                      onClick={() => void resetPacGroups()}
                      title={t("pac.resetGroupsHint")}
                    >
                      {t("pac.resetGroups")}
                    </GlassButton>
                    {pacSaving && (
                      <span className="muted" style={{ fontSize: 11 }}>
                        {t("pac.saving")}
                      </span>
                    )}
                  </div>

                  {pacGroups.map((g, gi) => (
                    <div
                      key={g.id}
                      className={`ruleset-item pac-group-item${
                        pacActiveGroup === g.id ? " selected" : ""
                      }`}
                      role="button"
                      tabIndex={0}
                      onClick={() => {
                        setPacActiveGroup(g.id);
                        setPacNewItem("");
                        setPacMenuGroup(null);
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          setPacActiveGroup(g.id);
                          setPacNewItem("");
                          setPacMenuGroup(null);
                        }
                      }}
                    >
                      <div className="ruleset-item-top">
                        <span className="ruleset-name">{g.label}</span>
                        {g.readOnly && (
                          <span className="ruleset-builtin-label">
                            {t("pac.readonly")}
                          </span>
                        )}
                        <span
                          className="ruleset-switch"
                          onClick={(e) => e.stopPropagation()}
                          onKeyDown={(e) => e.stopPropagation()}
                          role="presentation"
                        >
                          <GlassSwitchControl
                            checked={g.enabled}
                            size="sm"
                            title={
                              g.enabled
                                ? t("pac.disableGroup")
                                : t("pac.enableGroup")
                            }
                            onChange={(checked) =>
                              void togglePacGroup(g.id, checked)
                            }
                          />
                        </span>
                        <div className="rule-menu" data-pac-menu>
                          <button
                            type="button"
                            className="rule-menu-trigger"
                            aria-label={t("pac.menuAria", { name: g.label })}
                            aria-haspopup="menu"
                            aria-expanded={pacMenuGroup === g.id}
                            onClick={(e) => {
                              e.stopPropagation();
                              setPacMenuGroup((id) => (id === g.id ? null : g.id));
                            }}
                          >
                            ⋮
                          </button>
                          {pacMenuGroup === g.id && (
                            <div
                              className={`rule-menu-pop ruleset-menu-pop${
                                gi < Math.ceil(pacGroups.length / 2)
                                  ? " open-down"
                                  : ""
                              }`}
                              role="menu"
                            >
                              <button
                                type="button"
                                role="menuitem"
                                className="rule-menu-item"
                                onClick={() => {
                                  setPacMenuGroup(null);
                                  setPacEditGroup(g.id);
                                  setPacEditName(g.label);
                                  setPacEditRemoteUrl(g.remoteUrl);
                                  setPacEditAutoUpdate(g.autoUpdate);
                                  setPacEditInterval(g.intervalHours ?? 24);
                                  setPacEditOpen(true);
                                }}
                              >
                                {t("pac.editGroup")}
                              </button>
                              {!g.builtin && (
                                <button
                                  type="button"
                                  role="menuitem"
                                  className="rule-menu-item danger"
                                  onClick={() => {
                                    setPacMenuGroup(null);
                                    deletePacGroup(g.id);
                                  }}
                                >
                                  {t("pac.deleteGroup")}
                                </button>
                              )}
                              {g.remoteUrl && (
                                <button
                                  type="button"
                                  role="menuitem"
                                  className="rule-menu-item"
                                  disabled={pacRefreshing}
                                  onClick={() => {
                                    setPacMenuGroup(null);
                                    void onRefreshPacGroup(g.id);
                                  }}
                                >
                                  {pacRefreshing
                                    ? t("pac.refreshing")
                                    : t("pac.refresh")}
                                </button>
                              )}
                              {g.builtin && g.id === "gfwlist_domains" && (
                                <>
                                  <button
                                    type="button"
                                    role="menuitem"
                                    className="rule-menu-item"
                                    onClick={() => {
                                      setPacMenuGroup(null);
                                      void onPreviewPac();
                                    }}
                                  >
                                    {t("pac.preview")}
                                  </button>
                                  <button
                                    type="button"
                                    role="menuitem"
                                    className="rule-menu-item danger"
                                    disabled={pacClearing}
                                    onClick={() => {
                                      setPacMenuGroup(null);
                                      void onClearGfwlist();
                                    }}
                                  >
                                    {pacClearing ? t("pac.clearing") : t("pac.clear")}
                                  </button>
                                </>
                              )}
                            </div>
                          )}
                        </div>
                      </div>
                      <div className="muted" style={{ fontSize: 12 }}>
                        {t("pac.entryCount", { n: g.items.length })}
                      </div>
                    </div>
                  ))}
                </aside>

                <section className="rules-main">
                  <div className="rules-toolbar card">
                    <div className="header-actions rules-main-actions">
                      <div className="rules-policy-control">
                        <span className="muted rules-policy-label">
                          {t("pac.currentGroup")}
                        </span>
                        <span className="pill">{activePacGroup?.label}</span>
                        <span className="muted rules-policy-label"> · </span>
                        <span
                          className="mono pac-port-chip"
                          role="button"
                          tabIndex={0}
                          title={t("pac.portHint")}
                          onClick={() => setPacPortOpen(true)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter" || e.key === " ") {
                              e.preventDefault();
                              setPacPortOpen(true);
                            }
                          }}
                        >
                          {t("pac.port")}: {pacStatus?.port ?? "…"}
                        </span>
                      </div>
                      {!activePacGroup?.readOnly && (
                        <div className="rules-toolbar-tail">
                          <input
                            autoCapitalize="off"
                            autoCorrect="off"
                            spellCheck={false}
                            className="search rules-filter"
                            placeholder={activePacGroup?.placeholder}
                            value={pacNewItem}
                            onChange={(e) => setPacNewItem(e.target.value)}
                            onKeyDown={(e) => {
                              if (e.key === "Enter") {
                                e.preventDefault();
                                addPacEntry();
                              }
                            }}
                          />
                          <GlassButton
                            icon="+"
                            disabled={!pacNewItem.trim()}
                            onClick={addPacEntry}
                          >
                            {t("common.add")}
                          </GlassButton>
                        </div>
                      )}
                    </div>
                  </div>

                  {activePacGroup?.readOnly && (
                    <div
                      className="field-hint muted"
                      style={{ marginBottom: "0.75rem" }}
                    >
                      {t("pac.clearHint")}
                    </div>
                  )}

                  <div className="card table-wrap rules-table-wrap">
                    {!activePacGroup || activePacGroup.items.length === 0 ? (
                      <div className="empty muted">
                        {activePacGroup?.readOnly
                          ? t("pac.gfwlistEmpty")
                          : t("pac.emptyGroup")}
                      </div>
                    ) : (
                      <table className="rules-table">
                        <colgroup>
                          <col className="col-ord" />
                          <col />
                          <col className="col-actions" />
                        </colgroup>
                        <thead>
                          <tr>
                            <th>#</th>
                            <th>{t("pac.entry")}</th>
                            <th></th>
                          </tr>
                        </thead>
                        <tbody>
                          {activePacGroup.items.map((item, i) => (
                            <tr key={`${item}-${i}`} className="rule-row">
                              <td className="rule-ord">{i + 1}</td>
                              <td className="rule-payload" title={item}>
                                <code className="mono">{item}</code>
                              </td>
                              <td className="rule-actions-cell">
                                {!activePacGroup.readOnly && (
                                  <button
                                    type="button"
                                    className="icon-btn"
                                    aria-label={t("common.delete")}
                                    onClick={() => removePacEntry(i)}
                                  >
                                    ×
                                  </button>
                                )}
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    )}
                  </div>
                </section>
              </div>
            </>
          )}

          {pacEditOpen && (
            <div
              className="modal-backdrop"
              role="dialog"
              aria-modal="true"
              onClick={() => setPacEditOpen(false)}
            >
              <div
                className="modal pac-group-modal"
                onClick={(e) => e.stopPropagation()}
              >
                <header className="modal-header">
                  <h2>
                    {pacEditGroup === null
                      ? t("pac.newGroupTitle")
                      : t("pac.editGroupTitle")}
                  </h2>
                  <button
                    type="button"
                    className="icon-btn"
                    onClick={() => setPacEditOpen(false)}
                    aria-label={t("pac.close")}
                  >
                    ×
                  </button>
                </header>
                <div className="modal-body">
                  <label className="field">
                    <span>{t("pac.newGroupName")}</span>
                    <input
                      autoFocus
                      className="config-paste"
                      type="text"
                      spellCheck={false}
                      placeholder={t("pac.newGroupNamePh")}
                      value={pacEditName}
                      disabled={pacEditGroup !== null && BUILTIN_PAC_IDS.has(pacEditGroup)}
                      onChange={(e) => setPacEditName(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") {
                          e.preventDefault();
                          if (pacEditName.trim()) {
                            void savePacEdit(
                              pacEditGroup,
                              pacEditName,
                              pacEditRemoteUrl,
                              pacEditAutoUpdate,
                              pacEditInterval,
                            );
                          }
                        }
                      }}
                    />
                    {pacEditGroup !== null && BUILTIN_PAC_IDS.has(pacEditGroup) && (
                      <span className="field-hint muted">
                        {t("pac.builtinNameLocked")}
                      </span>
                    )}
                  </label>
                  <label className="field">
                    <span>{t("pac.remoteUrl")}</span>
                    <div className="pac-auto-row">
                      <SolidSelect
                        value={
                          pacPresets.find((p) => p.url === pacEditRemoteUrl)?.id ??
                          "custom"
                        }
                        options={[
                          ...pacPresets.map((p) => ({
                            value: p.id,
                            label: p.label,
                          })),
                          { value: "custom", label: t("pac.sourceCustom") },
                        ]}
                        onChange={(id) => {
                          const preset = pacPresets.find((p) => p.id === id);
                          if (preset) setPacEditRemoteUrl(preset.url);
                        }}
                      />
                      <input
                        className="config-paste mono"
                        type="text"
                        spellCheck={false}
                        placeholder="https://example.com/list.txt"
                        value={pacEditRemoteUrl}
                        onChange={(e) => setPacEditRemoteUrl(e.target.value)}
                      />
                    </div>
                    <span className="field-hint muted">
                      {t("pac.remoteUrlHint")}
                    </span>
                  </label>
                  <label className="field">
                    <span>{t("pac.autoUpdate")}</span>
                    <div className="pac-auto-row">
                      <GlassSwitchControl
                        checked={pacEditAutoUpdate}
                        ready={pacStatus !== null}
                        onChange={setPacEditAutoUpdate}
                      />
                      <SolidSelect
                        value={String(pacEditInterval)}
                        options={pacIntervalOptions}
                        disabled={!pacEditAutoUpdate || !pacEditRemoteUrl.trim()}
                        onChange={(v) => setPacEditInterval(Number(v))}
                      />
                    </div>
                    <span className="field-hint muted">
                      {t("pac.autoUpdateHint")}
                    </span>
                  </label>
                </div>
                <footer className="modal-footer">
                  <GlassButton onClick={() => setPacEditOpen(false)}>
                    {t("pac.cancel")}
                  </GlassButton>
                  <GlassButton
                    variant="primary"
                    disabled={!pacEditName.trim() || pacSettingsSaving}
                    onClick={() =>
                      void savePacEdit(
                        pacEditGroup,
                        pacEditName,
                        pacEditRemoteUrl,
                        pacEditAutoUpdate,
                        pacEditInterval,
                      )
                    }
                  >
                    {pacSettingsSaving
                      ? t("pac.saving")
                      : t("pac.confirm")}
                  </GlassButton>
                </footer>
              </div>
            </div>
          )}

          {pacPortOpen && (
            <div
              className="modal-backdrop"
              role="dialog"
              aria-modal="true"
              onClick={() => setPacPortOpen(false)}
            >
              <div
                className="modal pac-settings-modal"
                onClick={(e) => e.stopPropagation()}
              >
                <header className="modal-header">
                  <h2>{t("pac.portTitle")}</h2>
                  <button
                    type="button"
                    className="icon-btn"
                    onClick={() => setPacPortOpen(false)}
                    aria-label={t("pac.close")}
                  >
                    ×
                  </button>
                </header>
                <div className="modal-body pac-settings-body">
                  <label className="field">
                    <span>{t("pac.port")}</span>
                    <input
                      className="config-paste mono"
                      type="number"
                      min={1}
                      max={65535}
                      value={pacPort}
                      onChange={(e) => setPacPort(e.target.value)}
                    />
                    <span className="field-hint muted">{t("pac.portHint")}</span>
                  </label>
                </div>
                <footer className="modal-footer">
                  <GlassButton onClick={() => setPacPortOpen(false)}>
                    {t("pac.cancel")}
                  </GlassButton>
                  <GlassButton
                    variant="primary"
                    disabled={pacSettingsSaving}
                    onClick={async () => {
                      const port = Number.parseInt(pacPort, 10);
                      if (!Number.isFinite(port) || port < 1 || port > 65535) {
                        setPacError(t("pac.portInvalid"));
                        return;
                      }
                      setPacSettingsSaving(true);
                      setPacError(null);
                      try {
                        const status = await updatePacSettings({
                          sourceUrl: pacSourceUrl || pacStatus?.source_url || "",
                          autoUpdate: pacAutoUpdate,
                          updateIntervalHours: pacIntervalHours,
                          pacPort: port,
                        });
                        setPacStatus(status);
                        setPacPortOpen(false);
                      } catch (e) {
                        setPacError(
                          t("pac.settingsError", {
                            err: typeof e === "string" ? e : String(e),
                          }),
                        );
                      } finally {
                        setPacSettingsSaving(false);
                      }
                    }}
                  >
                    {pacSettingsSaving
                      ? t("pac.saving")
                      : t("pac.settingsSave")}
                  </GlassButton>
                </footer>
              </div>
            </div>
          )}

          {pacPreview !== null && (
            <div
              className="modal-backdrop"
              role="dialog"
              aria-modal="true"
              onClick={() => setPacPreview(null)}
            >
              <div
                className="modal pac-preview-modal"
                onClick={(e) => e.stopPropagation()}
              >
                <header className="modal-header">
                  <h2>{t("pac.previewTitle")}</h2>
                  <button
                    type="button"
                    className="icon-btn"
                    onClick={() => setPacPreview(null)}
                    aria-label={t("pac.close")}
                  >
                    ×
                  </button>
                </header>
                <div className="modal-body">
                  <pre className="pac-preview-text">
                    {pacPreviewLoading ? t("pac.previewLoading") : pacPreview}
                  </pre>
                </div>
                <footer className="modal-footer">
                  <GlassButton
                    onClick={() => void onCopyPac()}
                    disabled={pacPreviewLoading}
                  >
                    {pacCopied ? t("common.copied") : t("common.copy")}
                  </GlassButton>
                  <GlassButton variant="primary" onClick={() => setPacPreview(null)}>
                    {t("pac.close")}
                  </GlassButton>
                </footer>
              </div>
            </div>
          )}
        </section>
      )}

      {visibleTab === "core" && (
        <section className="settings-panel version-v3" aria-label="Version">
          {coreError && (
            <ErrorModal
              message={coreError}
              onClose={() => {
                setCoreError(null);
                clearCoreDownload();
              }}
            />
          )}

          <div className="version-core-grid">
            {renderCoreRow("singbox")}
            {renderCoreRow("xray")}
            {renderCoreRow("mihomo")}
          </div>

          <div className="card core-card app-card app-bar">
            <div className="app-bar-main">
              <div className="ver-mark app-mark" aria-hidden>
                ◈
              </div>
              <div className="app-bar-id">
                <span className="ver-name">Satelite</span>
                <span className="ver-sub muted">{t("settings.appTagline")}</span>
              </div>
              <div className="app-bar-ver">
                <span className="stat-label">{t("settings.coreCurrent")}</span>
                <span className="ver-ver mono">{appVersion ?? "…"}</span>
              </div>
              <div className="app-bar-latest">
                <span className="stat-label">{t("settings.appLatest")}</span>
                <span className="mono ver-stat">
                  {appChecking && !appUpdate
                    ? "…"
                    : (appUpdate?.latest_version ?? "—")}
                  {appUpdate?.update_available ? (
                    <span className="pill warn">
                      {t("settings.coreUpdateAvail")}
                    </span>
                  ) : appUpdate ? (
                    <span className="pill ok">{t("settings.appUpToDate")}</span>
                  ) : null}
                </span>
              </div>
              <div className="app-bar-actions">
                <GlassButton
                  icon="↻"
                  disabled={appChecking}
                  onClick={() => void runAppUpdateCheck(true, true)}
                >
                  {appChecking
                    ? t("settings.coreChecking")
                    : t("settings.coreCheck")}
                </GlassButton>
                {/* The app has no in-app downloader — "re-download" simply
                   opens the latest GitHub release page in the browser. */}
                <GlassButton
                  icon="⤓"
                  onClick={() => void openUrl(RELEASES_URL)}
                >
                  {t("settings.coreRedownload")}
                </GlassButton>
              </div>
            </div>

            {appError && (
              <ErrorModal
                message={appError}
                onClose={() => setAppError(null)}
              />
            )}

            {/* Quiet meta strip: last update check, host platform (the core
               binary targets it), build stack, and the exe path with copy. */}
            <div className="app-bar-foot">
              <span className="app-bar-meta">
                {t("settings.appCheckedLabel")}{" "}
                {appChecking ? "…" : formatCheckedAt(appUpdate?.checked_at)}
              </span>
              <span className="app-bar-meta">
                {t("settings.appPlatformLabel")}{" "}
                {cores.singbox?.platform ?? cores.xray?.platform ?? cores.mihomo?.platform ?? "—"}
              </span>
              <span className="app-bar-meta">Tauri · React · Rust</span>
              {appPath && (
                <>
                  <code className="kernel-path mono" title={appPath}>
                    {appPath}
                  </code>
                  <GlassButton
                    iconOnly
                    icon={copiedPath === appPath ? "✓" : "⧉"}
                    className="kernel-copy-btn"
                    title={copiedPath === appPath ? t("common.copied") : t("common.copy")}
                    aria-label={t("common.copy")}
                    onClick={() => void onCopyPath(appPath)}
                  />
                </>
              )}
            </div>

            <div className="ver-links">
              <button
                type="button"
                className="corner-link project-link"
                onClick={() => {
                  openUrl(PROJECT_URL).catch((e) =>
                    setError(typeof e === "string" ? e : String(e)),
                  );
                }}
              >
                {t("settings.projectHome")}
              </button>
            </div>
          </div>
        </section>
      )}

      {inboundOpen && (
        <div className="modal-backdrop">
          <div className="modal">
            <header className="modal-header">
              <h2>
                {inboundEditId
                  ? t("settings.editInboundTitle")
                  : t("settings.addInboundPort")}
              </h2>
              <button
                type="button"
                className="icon-btn"
                onClick={() => setInboundOpen(false)}
                disabled={busy}
                aria-label={t("common.cancel")}
              >
                ×
              </button>
            </header>
            <form
              className="modal-body"
              onSubmit={(e) => {
                e.preventDefault();
                void saveInbound();
              }}
            >
              <div className="field">
                <span>{t("settings.inboundType")}</span>
                <GlassSeg
                  value={inboundKind}
                  ariaLabel={t("settings.inboundType")}
                  disabled={busy}
                  onChange={(v) => setInboundKind(v as "mixed" | "http")}
                  options={[
                    { value: "mixed", label: "mixed" },
                    { value: "http", label: "http" },
                  ]}
                />
              </div>
              <label className="field">
                <span>{t("settings.portLabel")}</span>
                <input
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  value={inboundPort}
                  onChange={(e) => setInboundPort(e.target.value)}
                  placeholder="8080"
                  disabled={busy}
                  autoFocus
                />
              </label>
              <div className="via-proxy-row">
                <div>
                  <div className="sys-proxy-title">{t("settings.allowLan")}</div>
                  <div className="sys-proxy-desc">{t("settings.allowLanDesc")}</div>
                </div>
                <GlassSwitchControl
                  checked={inboundLan}
                  title={t("settings.allowLan")}
                  disabled={busy}
                  onChange={setInboundLan}
                />
              </div>
              {inboundError && <div className="form-error">{inboundError}</div>}
              <footer className="modal-footer">
                <GlassButton onClick={() => setInboundOpen(false)} disabled={busy}>
                  {t("common.cancel")}
                </GlassButton>
                <GlassButton type="submit" variant="primary" disabled={busy}>
                  {busy ? t("common.saving") : t("common.save")}
                </GlassButton>
              </footer>
            </form>
          </div>
        </div>
      )}

      {accentPickerOpen && (
        <AccentColorPickerModal
          current={
            isCustomHexAccent(accent) ? accent : resolveAccent(accent)[theme]
          }
          title={t("settings.accentCustomTitle")}
          applyLabel={t("common.save")}
          cancelLabel={t("common.cancel")}
          onApply={(hex) => {
            setAccentPickerOpen(false);
            void setAccent(hex);
          }}
          onClose={() => setAccentPickerOpen(false)}
        />
      )}

      {glowPickerOpen && (
        <AccentColorPickerModal
          current={
            isCustomHexAccent(glow)
              ? glow
              : resolveAccent(glow === "accent" ? accent : glow)[theme]
          }
          title={t("settings.glowCustomTitle")}
          applyLabel={t("common.save")}
          cancelLabel={t("common.cancel")}
          onPreview={(hex) => applyGlowToDom(hex, accent, theme)}
          onRestore={() => applyGlowToDom(glow, accent, theme)}
          onApply={(hex) => {
            setGlowPickerOpen(false);
            void setGlow(hex);
          }}
          onClose={() => setGlowPickerOpen(false)}
        />
      )}
      </div>
    </div>
  );
}

/** Order-sensitive equality for the extra-inbound draft list. */
function sameInbounds(a: ExtraInbound[], b: ExtraInbound[]) {
  if (a.length !== b.length) return false;
  return a.every((x, i) => {
    const y = b[i];
    return (
      x.id === y.id &&
      x.kind === y.kind &&
      x.port === y.port &&
      !!x.allow_lan === !!y.allow_lan
    );
  });
}

function AppToggle({
  title,
  desc,
  checked,
  disabled,
  onChange,
}: {
  title: string;
  desc: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="settings-app-row">
      <div className="settings-app-text">
        <div className="settings-app-title">{title}</div>
        <div className="settings-app-desc muted">{desc}</div>
      </div>
      <GlassSwitchControl
        checked={checked}
        title={title}
        disabled={disabled}
        onChange={onChange}
      />
    </div>
  );
}
