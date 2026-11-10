<!--
  The tree's context menu (file-tree.md: right-click, Shift-F10, the Menu key; components.md menu
  pattern). `role="menu"` with `menuitem`s; focus moves into the menu and an item is active through
  `aria-activedescendant`. Unavailable entries are Disabled, not hidden: greyed, `aria-disabled`,
  with a tooltip saying why; they stay navigable but never activate.
  Keys: Up/Down (wrapping), Home/End, Enter/Space activate, Esc closes (the host returns focus to the
  row), Tab closes. A press outside closes it.
-->
<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";

export interface MenuItem {
  id: string;
  label: string;
  enabled: boolean;
  /** Why it is unavailable (the tooltip on a disabled entry). */
  reason?: string;
  /** The accelerator as display chips for this platform ("⌘", "⇧", "N" / "Ctrl", "Shift", "N"). */
  accel?: string[];
  /** The same accelerator spoken ("Command", "Shift", "N"): what a screen reader gets instead of glyphs. */
  spoken?: string[];
}

const props = defineProps<{ items: readonly MenuItem[]; x: number; y: number; label: string }>();
const emit = defineEmits<{ select: [id: string]; close: [reason: "escape" | "tab" | "outside"] }>();

const root = ref<HTMLElement | null>(null);
const active = ref(Math.max(0, props.items.findIndex((i) => i.enabled)));

function activate(index: number): void {
  const item = props.items[index];
  if (item?.enabled) emit("select", item.id);
}

function onKeydown(event: KeyboardEvent): void {
  const n = props.items.length;
  switch (event.key) {
    case "ArrowDown":
      active.value = (active.value + 1) % n;
      break;
    case "ArrowUp":
      active.value = (active.value - 1 + n) % n;
      break;
    case "Home":
      active.value = 0;
      break;
    case "End":
      active.value = n - 1;
      break;
    case "Enter":
    case " ":
      activate(active.value);
      break;
    case "Escape":
      emit("close", "escape");
      break;
    case "Tab":
      emit("close", "tab");
      return; // Tab moves on: do not trap or swallow it
    default:
      return;
  }
  event.preventDefault();
  event.stopPropagation();
}

function onOutside(event: MouseEvent): void {
  if (root.value && !root.value.contains(event.target as Node)) emit("close", "outside");
}

onMounted(() => {
  root.value?.focus();
  document.addEventListener("mousedown", onOutside, true);
});
onBeforeUnmount(() => document.removeEventListener("mousedown", onOutside, true));
</script>

<template>
  <div
    ref="root"
    class="serat-tree-menu"
    role="menu"
    tabindex="-1"
    :aria-label="label"
    :aria-activedescendant="`serat-tree-menu-${active}`"
    :style="{ left: x + 'px', top: y + 'px' }"
    data-testid="tree-context-menu"
    @keydown="onKeydown"
  >
    <div
      v-for="(item, i) in items"
      :id="`serat-tree-menu-${i}`"
      :key="item.id"
      class="serat-tree-menu-item"
      :class="{ active: i === active, disabled: !item.enabled }"
      role="menuitem"
      :aria-disabled="!item.enabled"
      :title="!item.enabled ? item.reason : undefined"
      :aria-description="!item.enabled ? item.reason : undefined"
      :data-item="item.id"
      @mousemove="active = i"
      @click="activate(i)"
    >
      <span class="serat-tree-menu-label">{{ item.label }}</span>
      <span v-if="item.accel?.length" class="serat-tree-menu-accel" aria-hidden="true" data-accel>{{ item.accel.join(" ") }}</span>
      <span v-if="item.spoken?.length" class="serat-tree-menu-spoken" data-spoken>, shortcut {{ item.spoken.join(" ") }}</span>
    </div>
  </div>
</template>

<style scoped>
.serat-tree-menu {
  position: fixed;
  z-index: 20;
  min-width: 12rem;
  padding: 4px 0;
  outline: none;
  background: var(--serat-surface-raised, #fff);
  border: 1px solid var(--serat-border-hairline, #bbb);
}
.serat-tree-menu-item {
  display: flex;
  justify-content: space-between;
  gap: 1.5rem;
  padding: 4px 12px;
  cursor: pointer;
}
.serat-tree-menu-accel {
  color: var(--serat-text-secondary, #6b7075);
}
.serat-tree-menu-spoken {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
}
.serat-tree-menu-item.active {
  background: var(--serat-surface-selected, rgba(36, 69, 126, 0.12));
}
.serat-tree-menu-item.disabled {
  color: var(--serat-text-secondary, #6b7075);
  cursor: default;
}
</style>
