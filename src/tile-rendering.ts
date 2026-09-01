import { uiIcon } from "./icons";
import { escapeHtml } from "./html";
import { portBadgeLabels } from "./presentation";
import type { ServiceSnapshot } from "./types";

export type SharedServiceCardOptions = {
  category: string;
  cardClass?: string;
  metricsId?: string;
  cardAttributes?: string;
  ariaLabel: string;
  selected?: boolean;
  busy?: boolean;
  ordinal?: number;
  ordinalTotal?: number;
  iconMarkup: string;
  pipClass: string;
  title: string;
  overlayMarkup?: string;
  controlsMarkup?: string;
  metricsMarkup: string;
  ports: number[];
  emptyPortLabel: string;
  trailingMarkup?: string;
};

export function renderSharedServiceCard(options: SharedServiceCardOptions): string {
  const cardClasses = `service-tile category-${options.category}${options.cardClass ? ` ${options.cardClass}` : ""}${options.busy ? " is-busy" : ""}${options.selected ? " is-selected" : ""}`;
  const metricsAttribute = options.metricsId ? ` data-metrics-id="${escapeHtml(options.metricsId)}"` : "";
  return `
    <article class="${cardClasses}"${metricsAttribute}${options.cardAttributes ?? ""} aria-label="${escapeHtml(options.ariaLabel)}">
      ${options.overlayMarkup ?? ""}
      <div class="tile-top">
        <span class="icon-well service-icon" aria-hidden="true">${renderTileOrdinal(options.ordinal, options.ordinalTotal)}${options.iconMarkup}<span class="status-pip state-${options.pipClass}"></span></span>
        ${renderTileHeading(options.title)}
        ${options.controlsMarkup ? `<div class="service-card-actions">${options.controlsMarkup}</div>` : ""}
      </div>
      <div class="tile-metrics">${options.metricsMarkup}</div>
      ${renderTileFoot(options.ports, options.emptyPortLabel, options.trailingMarkup ?? "")}
    </article>`;
}

export function renderOpenServiceButton(service: ServiceSnapshot, label: string): string {
  if (!service.browser_url) return "";
  return `<button type="button" class="service-link icon-only-button service-card-control" data-tile-action data-action="open-service" data-service-id="${escapeHtml(service.id)}" aria-label="Open ${escapeHtml(label)} in the browser" title="Open ${escapeHtml(service.browser_url)}">${uiIcon("external", 15)}</button>`;
}

/** The number of cards a group holds, shown next to its name so the tab count adds up on screen. */
export function renderGroupCount(count: number): string {
  return `<span class="section-count">${count}</span>`;
}

export function renderTileOrdinal(ordinal?: number, total?: number): string {
  if (!ordinal || !total || total < 2) return "";
  return `<span class="tile-ordinal" title="Item ${ordinal} of ${total}" aria-label="Item ${ordinal} of ${total}">${ordinal}</span>`;
}

// The subtitle carried technology labels in an earlier card layout. Cards now use
// the title alone, so keeping this helper focused prevents empty placeholder markup.
export function renderTileHeading(title: string): string {
  return `<div class="tile-heading">
        <h3 class="tile-name" title="${escapeHtml(title)}">${escapeHtml(title)}</h3>
      </div>`;
}

export function renderTileFoot(ports: number[], emptyLabel: string, trailing: string): string {
  const labels = portBadgeLabels(ports);
  return `<div class="tile-foot">
        <div class="port-row">
          ${labels.map((label) => `<span class="port-chip${label.startsWith("+") ? " port-overflow" : ""}" title="${escapeHtml(portChipDescription(label, ports))}" aria-label="${escapeHtml(portChipDescription(label, ports))}">${escapeHtml(label)}</span>`).join("")}
          ${ports.length === 0 ? `<span class="no-port-label port-empty-icon" title="${escapeHtml(emptyLabel)}" aria-label="${escapeHtml(emptyLabel)}">${uiIcon("port", 14)}</span>` : ""}
        </div>
        ${trailing}
      </div>`;
}

function portChipDescription(label: string, ports: number[]): string {
  if (!label.startsWith("+")) return `Listening port ${label}`;
  return `${ports.length - 1} additional listening ports: ${ports.slice(1).join(", ")}`;
}
