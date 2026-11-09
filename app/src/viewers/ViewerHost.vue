<!--
  Binary viewer host (FR-BIN-11, FR-BIN-12, US-19; sidecar action FR-BIN-02, US-18; binary-viewers.md). Occupies the editor's
  place. Reads `asset_info` first (size and MIME before any byte is requested), then hands the
  `assetUrl` of the path to the image or PDF viewer. The sidecar's metadata is reached through
  "Edit metadata note", which opens `x.pdf.md` as an ordinary note in the editor (creating it
  when absent): this component never edits it.
-->
<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";

import { assetUrl } from "@/editor/assetUrl";
import { assetInfo, isIpcError, writeFile } from "@/ipc/client";
import type { AssetInfo } from "@/ipc/generated/AssetInfo";

import ImageViewer from "./ImageViewer.vue";
import PdfViewer from "./PdfViewer.vue";
import { IMAGE_SERVE_CAP, openKind, sidecarPathFor } from "./assetKind";
import type { PdfLoader } from "./pdfEngine";

const props = defineProps<{ path: string; pdfLoader?: PdfLoader }>();
const emit = defineEmits<{ (e: "open-note", path: string): void; (e: "reveal", path: string): void }>();

type State = "loading" | "ready" | "missing" | "unsupported" | "too-large" | "error";
const state = ref<State>("loading");
const info = ref<AssetInfo | null>(null);
const url = ref<string | null>(null);
const dims = ref<{ w: number; h: number } | null>(null);
const metaOpen = ref(true);
const note = ref<string | null>(null);
/** Page to resume at after a restart, and the 4 s "changed on disk" notice (spec state table). */
const resumePage = ref(1);
const changedNotice = ref(false);
let noticeTimer: ReturnType<typeof setTimeout> | null = null;

const name = computed(() => props.path.slice(props.path.lastIndexOf("/") + 1));
const kind = computed(() => openKind(props.path));

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / 1024 / 1024).toFixed(1)} MiB`;
}

async function load(path: string, restart = false): Promise<void> {
  if (!restart) resumePage.value = 1;
  const openedBytes = restart ? (info.value?.bytes ?? null) : null;
  state.value = "loading";
  info.value = null;
  url.value = null;
  dims.value = null;
  note.value = null;
  if (!restart) changedNotice.value = false;
  let result: AssetInfo;
  try {
    result = await assetInfo({ path });
  } catch (error) {
    if (props.path !== path) return;
    state.value = isIpcError(error) && error.kind === "not_found" ? "missing" : "error";
    return;
  }
  if (props.path !== path) return; // a later open superseded this one
  info.value = result;
  if (openedBytes !== null && result.bytes === openedBytes) {
    // A restart with nothing different on disk would just fail again: say so instead of looping.
    state.value = "error";
    return;
  }
  if (!result.viewable || (kind.value !== "pdf" && kind.value !== "image")) {
    state.value = "unsupported";
    return;
  }
  if (kind.value === "image" && result.bytes > IMAGE_SERVE_CAP) {
    state.value = "too-large";
    return;
  }
  try {
    url.value = assetUrl(path); // the one builder; refuses traversal/empty/ignored-dir paths
  } catch {
    state.value = "error";
    return;
  }
  state.value = "ready";
  if (restart) {
    changedNotice.value = true;
    if (noticeTimer) clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (changedNotice.value = false), 4000);
  }
}

/** The PDF's bytes no longer match what was opened: re-read asset_info and restart the load. */
function onPdfChanged(page: number): void {
  resumePage.value = page;
  void load(props.path, true);
}

onBeforeUnmount(() => {
  if (noticeTimer) clearTimeout(noticeTimer);
});

watch(() => props.path, (p) => void load(p), { immediate: true });

/** FR-BIN-02: open the sidecar as a normal note; create it (empty frontmatter) when absent. */
async function editMetadata(): Promise<void> {
  const sidecar = info.value?.sidecar_path ?? null;
  if (sidecar) {
    emit("open-note", sidecar);
    return;
  }
  const target = sidecarPathFor(props.path);
  try {
    await writeFile({ path: target, text: "---\n\n---\n", base_hash: null });
  } catch (error) {
    // StaleHash on a create means it already exists (made elsewhere): just open it.
    if (!(isIpcError(error) && error.kind === "stale_hash")) {
      note.value = "The metadata note could not be created";
      return;
    }
  }
  emit("open-note", target);
}
</script>

<template>
  <section class="viewer-host" data-testid="viewer-host">
    <header class="meta" data-testid="viewer-header">
      <span class="name" data-testid="viewer-name">{{ name }}</span>
      <span v-if="info" class="mono info" data-testid="viewer-info"
        >{{ info.mime }} · {{ formatBytes(info.bytes) }}<template v-if="dims"> · {{ dims.w }}×{{ dims.h }}</template></span
      >
      <span class="spacer"></span>
      <button type="button" class="fold" :aria-expanded="metaOpen" data-testid="viewer-fold" @click="metaOpen = !metaOpen">
        Metadata
      </button>
    </header>
    <div v-if="metaOpen && state !== 'missing'" class="sidecar" data-testid="viewer-sidecar">
      <span v-if="info?.sidecar_path" class="mono" data-testid="viewer-sidecar-path">{{ info.sidecar_path }}</span>
      <span v-else class="hint">No metadata note yet.</span>
      <button type="button" data-testid="viewer-edit-meta" :disabled="!info" @click="editMetadata">
        {{ info?.sidecar_path ? "Edit metadata note" : "Create metadata note" }}
      </button>
      <span v-if="note" role="status" class="hint" data-testid="viewer-note">{{ note }}</span>
    </div>

    <p v-if="changedNotice" class="notice" role="status" data-testid="viewer-changed">
      This file changed on disk — showing the latest version.
    </p>
    <div class="body">
      <p v-if="state === 'loading'" class="status" data-testid="viewer-loading">Loading…</p>
      <p v-else-if="state === 'missing'" class="status" role="alert" data-testid="viewer-missing">
        This file is no longer on disk.
      </p>
      <p v-else-if="state === 'error'" class="status" role="alert" data-testid="viewer-error">
        This file couldn't be opened.
      </p>
      <div v-else-if="state === 'unsupported'" class="status" data-testid="viewer-unsupported">
        <p>This file type can't be previewed.</p>
        <p v-if="info" class="mono micro">{{ info.mime }}</p>
      </div>
      <div v-else-if="state === 'too-large'" class="status" role="alert" data-testid="viewer-too-large">
        <p>This image couldn't be displayed</p>
        <p v-if="info" class="mono micro">{{ info.mime }} · {{ formatBytes(info.bytes) }} is over the 64 MiB limit</p>
        <button type="button" @click="emit('reveal', path)">Reveal in file tree</button>
      </div>
      <ImageViewer
        v-else-if="state === 'ready' && kind === 'image' && url && info"
        :url="url"
        :name="name"
        :mime="info.mime"
        :bytes="info.bytes"
        @reveal="emit('reveal', path)"
        @dimensions="(d) => (dims = d)"
      />
      <PdfViewer
        v-else-if="state === 'ready' && kind === 'pdf' && url && info"
        :path="path"
        :length="info.bytes"
        :name="name"
        :loader="pdfLoader"
        :start-page="resumePage"
        @changed="onPdfChanged"
      />
    </div>
  </section>
</template>

<style scoped>
.viewer-host { display: flex; flex-direction: column; height: 100%; min-height: 0; }
.meta { display: flex; align-items: baseline; gap: 0.75rem; padding: 0.35rem 0.75rem; border-bottom: 1px solid #0002; }
.name { font-weight: 600; }
.info, .hint, .micro { color: #0009; font-size: 0.8rem; }
.mono { font-family: ui-monospace, "SF Mono", Menlo, monospace; }
.spacer { flex: 1; }
.sidecar { display: flex; align-items: center; gap: 0.75rem; padding: 0.3rem 0.75rem; border-bottom: 1px solid #0002; }
.notice { margin: 0; padding: 0.25rem 0.75rem; color: #0009; font-size: 0.85rem; }
.body { flex: 1; min-height: 0; }
.status { padding: 2rem; text-align: center; color: #0009; }
button { border: 1px solid #0003; background: transparent; border-radius: 3px; padding: 0.1rem 0.6rem; cursor: pointer; }
</style>
