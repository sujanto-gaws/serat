// §9.3: a rendered link never navigates the webview, whatever button fires it.

import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import Preview from "@/editor/Preview.vue";

function mountWithAnchor(attrs: string) {
  const wrapper = mount(Preview, { props: { source: "" }, attachTo: document.body });
  const host = wrapper.get('[data-testid="serat-preview"]').element as HTMLElement;
  host.innerHTML = `<a href="https://example.com" ${attrs}>x</a>`;
  return { wrapper, anchor: host.querySelector("a")! };
}

describe("Preview link interception", () => {
  it("prevents the default of a middle-click (auxclick) and does not open the link", () => {
    const { wrapper, anchor } = mountWithAnchor('data-external="true"');
    const event = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 });
    anchor.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(wrapper.emitted("external-link-click")).toBeUndefined();
    wrapper.unmount();
  });

  it("a primary click on an external link is prevented and emitted", () => {
    const { wrapper, anchor } = mountWithAnchor('data-external="true"');
    const event = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    anchor.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(wrapper.emitted("external-link-click")).toEqual([["https://example.com"]]);
    wrapper.unmount();
  });
});
