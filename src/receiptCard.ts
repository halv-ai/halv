import { formatUsdCompact } from "./format.ts";
import { calculateInputSavings } from "./savings.ts";
import { buildSavingsSparkline } from "./savingsSparkline.ts";
import type {
  AccountAuthStatus,
  DailyTokenActivity,
  LaneUsage,
  SavingsTotals,
} from "./types.ts";
import { summarizeSavingsByLane } from "./usageByLane.ts";

const CARD_WIDTH = 1080;
const CARD_HEIGHT = 1080;
const DAYS_IN_PERIOD = 7;

export type ReceiptData = {
  periodStart: string;
  periodEnd: string;
  tokensSaved: number;
  /** Dollars saved in the period (ADR 0015); null before the dollar metric. */
  savingsUsd: number | null;
  percentSaved: number | null;
  daily: number[];
  /** Dollars saved per day; the bars use them when the period has dollars. */
  dailyUsd: number[];
  agents: { label: string; tokensSaved: number; percent: number }[];
  receiptId: string | null;
  inviteCode: string | null;
};

export function getReceiptCardLayout() {
  const contentLeft = 80;
  const contentRight = CARD_WIDTH - contentLeft;

  return {
    card: { width: CARD_WIDTH, height: CARD_HEIGHT, radius: 28 },
    content: { left: contentLeft, right: contentRight },
    header: { baseline: 106 },
    headline: {
      valueBaseline: 426,
      labelBaseline: 510,
      summaryBaseline: 570,
      summaryLineHeight: 37,
      summaryWidth: 850,
    },
    dailyBars: {
      x: contentLeft,
      baseline: 840,
      barWidth: 118,
      gap: 16,
      radius: 5,
      scale: 4.17,
      dayBaseline: 875,
    },
    footer: {
      x: contentLeft,
      rightX: contentRight,
      dividerY: 914,
      baseline: 977,
      inviteX: 650,
      inviteY: 940,
      inviteWidth: 350,
      inviteHeight: 60,
    },
  };
}

export type ReceiptTheme = {
  accent: string;
  accentContrast: string;
  bg: string;
  border: string;
  fg: string;
  muted: string;
  secondary: string;
};

type BuildReceiptDataInput = {
  dailyTokenActivity: readonly DailyTokenActivity[];
  usageByLane: readonly LaneUsage[];
  weeklyTokensSaved: number | null;
  weeklyDollarsUsd?: number | null;
  totals: SavingsTotals | null;
  account: AccountAuthStatus | null;
  now: Date;
};

export function formatReceiptTokens(value: number): string {
  const safeValue = finiteNonnegative(value);
  if (safeValue < 1_000) {
    return String(Math.round(safeValue));
  }

  const units = [
    { threshold: 1_000_000_000, suffix: "B" },
    { threshold: 1_000_000, suffix: "M" },
    { threshold: 1_000, suffix: "K" },
  ];
  const unit = units.find(({ threshold }) => safeValue >= threshold) ?? units[2];
  const scaled = safeValue / unit.threshold;
  const digits = scaled >= 100 || Number.isInteger(scaled) ? 0 : 1;
  return `${scaled.toFixed(digits).replace(/\.0$/, "")}${unit.suffix}`;
}

export function formatReceiptPeriod(periodStart: string, periodEnd: string) {
  const start = dateFromKey(periodStart);
  const end = dateFromKey(periodEnd);
  const startText = start.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
  });
  const endText = end.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
  });
  return `${startText} \u2013 ${endText}`.toUpperCase();
}

/** The receipt is a shared image in English, so it uses US formats. */
export function formatReceiptUsd(value: number): string {
  return formatUsdCompact(finiteNonnegative(value), "en-US");
}

export function receiptHasDollars(data: ReceiptData): boolean {
  return data.savingsUsd !== null && data.savingsUsd > 0;
}

export function formatReceiptSummary(data: ReceiptData) {
  const kept = receiptHasDollars(data)
    ? `${formatReceiptTokens(data.tokensSaved)} tokens kept out of context, at API prices. `
    : "Cached once, reused all week. ";
  const traffic =
    data.percentSaved === null
      ? "My agent traffic ran through halv"
      : `${Math.round(data.percentSaved)}% of my agent traffic ran through halv`;
  const agents = data.agents
    .slice(0, 2)
    .map((agent) => `${agent.label} ${Math.round(agent.percent)}%`)
    .join(", ");
  const breakdown = agents ? ` \u2014 ${agents}` : "";
  return `${kept}${traffic}${breakdown}.`;
}

export function buildReceiptData({
  dailyTokenActivity,
  usageByLane,
  weeklyTokensSaved,
  weeklyDollarsUsd = null,
  account,
  now,
}: BuildReceiptDataInput): ReceiptData {
  const periodEndDate = localDay(now);
  const periodStartDate = addLocalDays(periodEndDate, -(DAYS_IN_PERIOD - 1));
  const periodStart = dateKey(periodStartDate);
  const periodEnd = dateKey(periodEndDate);
  const activityByDate = new Map(
    dailyTokenActivity.map((activity) => [activity.date, activity]),
  );
  const periodActivity = Array.from({ length: DAYS_IN_PERIOD }, (_, index) =>
    activityByDate.get(dateKey(addLocalDays(periodStartDate, index))),
  );
  const daily = periodActivity.map((activity) =>
    finiteNonnegative(activity?.tokensSaved ?? 0),
  );
  const tokensSaved = finiteNonnegative(
    weeklyTokensSaved ?? daily.reduce((total, value) => total + value, 0),
  );
  const compressedInputTokens = periodActivity.reduce(
    (total, activity) => total + finiteNonnegative(activity?.inputTokens ?? 0),
    0,
  );
  const inputSavings = calculateInputSavings(compressedInputTokens, tokensSaved);
  const dailyUsd = periodActivity.map((activity) =>
    finiteNonnegative(activity?.dollarsUsd ?? 0),
  );
  // The period's own dollars. Old savings have no dollar method, so a
  // period before the dollar metric has no dollar value.
  const periodUsd = finiteNonnegative(
    weeklyDollarsUsd ?? dailyUsd.reduce((total, value) => total + value, 0),
  );
  const savingsUsd = periodUsd > 0 ? periodUsd : null;

  return {
    periodStart,
    periodEnd,
    tokensSaved,
    savingsUsd,
    percentSaved: inputSavings?.percent ?? null,
    daily,
    dailyUsd,
    agents: summarizeSavingsByLane(usageByLane)
      .slice(0, 4)
      .map(({ label, tokensSaved: laneTokensSaved, percent }) => ({
        label,
        tokensSaved: laneTokensSaved,
        percent,
      })),
    receiptId: null,
    inviteCode:
      account?.referral?.code ?? account?.entitlements?.referral?.code ?? null,
  };
}

export function drawReceiptCard(
  canvas: HTMLCanvasElement,
  data: ReceiptData,
  theme: ReceiptTheme,
) {
  const layout = getReceiptCardLayout();
  const ratio = Math.max(1, window.devicePixelRatio || 1);
  canvas.width = Math.round(layout.card.width * ratio);
  canvas.height = Math.round(layout.card.height * ratio);
  canvas.style.aspectRatio = `${layout.card.width} / ${layout.card.height}`;
  canvas.style.width = `${layout.card.width}px`;
  canvas.style.height = "auto";

  const context = canvas.getContext("2d");
  if (!context) {
    return;
  }
  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  context.clearRect(0, 0, layout.card.width, layout.card.height);

  drawRoundedRectPath(
    context,
    0.5,
    0.5,
    layout.card.width - 1,
    layout.card.height - 1,
    layout.card.radius,
  );
  context.save();
  context.clip();
  context.fillStyle = theme.bg;
  context.fillRect(0, 0, layout.card.width, layout.card.height);
  const glow = context.createRadialGradient(
    layout.card.width,
    layout.card.height,
    0,
    layout.card.width,
    layout.card.height,
    560,
  );
  glow.addColorStop(0, colorWithAlpha(theme.accent, 0.1));
  glow.addColorStop(1, colorWithAlpha(theme.accent, 0));
  context.fillStyle = glow;
  context.fillRect(0, 0, layout.card.width, layout.card.height);
  context.restore();
  context.strokeStyle = colorWithAlpha(theme.border, 0.28);
  context.lineWidth = 1;
  drawRoundedRectPath(
    context,
    0.5,
    0.5,
    layout.card.width - 1,
    layout.card.height - 1,
    layout.card.radius,
  );
  context.stroke();

  context.textBaseline = "alphabetic";
  context.fillStyle = theme.fg;
  context.font = "500 24px ui-monospace, SFMono-Regular, Menlo, monospace";
  drawLetterSpacedText(
    context,
    "HALV",
    layout.content.left,
    layout.header.baseline,
    8,
  );
  context.textAlign = "right";
  context.font = "500 20px ui-monospace, SFMono-Regular, Menlo, monospace";
  context.fillStyle = theme.muted;
  context.fillText(
    formatReceiptPeriod(data.periodStart, data.periodEnd),
    layout.content.right,
    layout.header.baseline,
  );

  context.textAlign = "left";
  context.fillStyle = theme.accent;
  context.font = "700 248px Arial, ui-sans-serif, system-ui, sans-serif";
  const dollars = receiptHasDollars(data);
  context.fillText(
    dollars
      ? formatReceiptUsd(data.savingsUsd ?? 0)
      : formatReceiptTokens(data.tokensSaved),
    layout.content.left,
    layout.headline.valueBaseline,
    layout.content.right - layout.content.left,
  );
  context.fillStyle = theme.fg;
  context.font = "700 48px ui-sans-serif, system-ui, sans-serif";
  context.fillText(
    dollars ? "saved with Halv this week" : "tokens saved this week",
    layout.content.left,
    layout.headline.labelBaseline,
  );
  context.fillStyle = theme.secondary;
  context.font = "400 31px ui-sans-serif, system-ui, sans-serif";
  drawWrappedText(
    context,
    formatReceiptSummary(data),
    layout.content.left,
    layout.headline.summaryBaseline,
    layout.headline.summaryWidth,
    layout.headline.summaryLineHeight,
    2,
  );

  drawDailyBars(
    context,
    data.periodStart,
    dollars ? data.dailyUsd : data.daily,
    dollars ? formatReceiptUsd : formatReceiptTokens,
    theme,
    layout,
  );

  context.textAlign = "left";
  context.strokeStyle = colorWithAlpha(theme.border, 0.24);
  context.lineWidth = 1;
  context.beginPath();
  context.moveTo(layout.content.left, layout.footer.dividerY);
  context.lineTo(layout.content.right, layout.footer.dividerY);
  context.stroke();

  context.fillStyle = theme.accent;
  context.beginPath();
  context.arc(layout.footer.x + 5, layout.footer.baseline - 8, 5, 0, Math.PI * 2);
  context.fill();
  context.fillStyle = theme.fg;
  context.font = "500 22px ui-monospace, SFMono-Regular, Menlo, monospace";
  context.fillText(
    data.receiptId ? `halv.ai/r/${data.receiptId}` : "halv.ai",
    layout.footer.x + 24,
    layout.footer.baseline,
  );
  if (data.inviteCode) {
    context.fillStyle = theme.accent;
    drawRoundedRectPath(
      context,
      layout.footer.inviteX,
      layout.footer.inviteY,
      layout.footer.inviteWidth,
      layout.footer.inviteHeight,
      layout.footer.inviteHeight / 2,
    );
    context.fill();
    context.fillStyle = theme.accentContrast;
    context.font = "500 19px ui-monospace, SFMono-Regular, Menlo, monospace";
    context.textAlign = "left";
    drawCenteredLetterSpacedText(
      context,
      `INVITE CODE ${data.inviteCode.toUpperCase()}`,
      layout.footer.inviteX + layout.footer.inviteWidth / 2,
      layout.footer.inviteY + 38,
      2,
    );
  }
}

type ReceiptCardLayout = ReturnType<typeof getReceiptCardLayout>;

function drawDailyBars(
  context: CanvasRenderingContext2D,
  periodStart: string,
  daily: readonly number[],
  formatValue: (value: number) => string,
  theme: ReceiptTheme,
  layout: ReceiptCardLayout,
) {
  const series = buildSavingsSparkline([...daily], DAYS_IN_PERIOD);
  const highlightIndex = series.bars.reduce(
    (bestIndex, bar, index, bars) =>
      bestIndex < 0 || bar.value > bars[bestIndex].value ? index : bestIndex,
    -1,
  );
  const highlightedValue = series.bars[highlightIndex]?.value ?? 0;
  const start = dateFromKey(periodStart);

  series.bars.forEach((bar, index) => {
    const height = bar.height * layout.dailyBars.scale;
    const x =
      layout.dailyBars.x +
      index * (layout.dailyBars.barWidth + layout.dailyBars.gap);
    const highlighted = index === highlightIndex && highlightedValue > 0;
    context.fillStyle = theme.accent;
    const mutedBarAlpha = isDarkColor(theme.bg) ? 0.58 : 0.2;
    context.globalAlpha = highlighted ? 1 : mutedBarAlpha;
    drawRoundedRectPath(
      context,
      x,
      layout.dailyBars.baseline - height,
      layout.dailyBars.barWidth,
      height,
      layout.dailyBars.radius,
    );
    context.fill();
    context.globalAlpha = 1;

    const day = addLocalDays(start, index)
      .toLocaleDateString("en-US", { weekday: "short" })
      .toUpperCase();
    context.textAlign = "center";
    context.fillStyle = highlighted ? theme.accent : theme.muted;
    context.font = "500 13px ui-monospace, SFMono-Regular, Menlo, monospace";
    context.fillText(
      day,
      x + layout.dailyBars.barWidth / 2,
      layout.dailyBars.dayBaseline,
    );
    if (highlighted) {
      context.fillText(
        formatValue(bar.value),
        x + layout.dailyBars.barWidth / 2,
        layout.dailyBars.baseline - height - 11,
      );
    }
  });
  context.globalAlpha = 1;
}

function drawLetterSpacedText(
  context: CanvasRenderingContext2D,
  text: string,
  x: number,
  y: number,
  spacing: number,
) {
  let cursor = x;
  for (const character of text) {
    context.fillText(character, cursor, y);
    cursor += context.measureText(character).width + spacing;
  }
}

function drawCenteredLetterSpacedText(
  context: CanvasRenderingContext2D,
  text: string,
  centerX: number,
  y: number,
  spacing: number,
) {
  const width = Array.from(text).reduce(
    (total, character, index) =>
      total + context.measureText(character).width + (index === 0 ? 0 : spacing),
    0,
  );
  drawLetterSpacedText(context, text, centerX - width / 2, y, spacing);
}

function drawWrappedText(
  context: CanvasRenderingContext2D,
  text: string,
  x: number,
  firstBaseline: number,
  maxWidth: number,
  lineHeight: number,
  maxLines: number,
) {
  const words = text.split(/\s+/);
  const lines: string[] = [];
  let line = "";
  for (const word of words) {
    const candidate = line ? `${line} ${word}` : word;
    if (line && context.measureText(candidate).width > maxWidth) {
      lines.push(line);
      line = word;
    } else {
      line = candidate;
    }
  }
  if (line) lines.push(line);
  const visibleLines = lines.slice(0, maxLines);
  if (lines.length > maxLines) {
    let last = visibleLines[maxLines - 1] ?? "";
    while (last && context.measureText(`${last}\u2026`).width > maxWidth) {
      last = last.slice(0, -1).trimEnd();
    }
    visibleLines[maxLines - 1] = `${last}\u2026`;
  }
  visibleLines.forEach((visibleLine, index) => {
    context.fillText(visibleLine, x, firstBaseline + index * lineHeight);
  });
}

function drawRoundedRectPath(
  context: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
  radius: number,
) {
  const safeRadius = Math.max(0, Math.min(radius, width / 2, height / 2));
  context.beginPath();
  context.moveTo(x + safeRadius, y);
  context.arcTo(x + width, y, x + width, y + height, safeRadius);
  context.arcTo(x + width, y + height, x, y + height, safeRadius);
  context.arcTo(x, y + height, x, y, safeRadius);
  context.arcTo(x, y, x + width, y, safeRadius);
  context.closePath();
}

function colorWithAlpha(color: string, alpha: number) {
  const rgb = color.match(/[\d.]+/g)?.slice(0, 3).join(" ");
  return rgb ? `rgb(${rgb} / ${alpha})` : color;
}

function isDarkColor(color: string) {
  const [red = 255, green = 255, blue = 255] =
    color.match(/[\d.]+/g)?.slice(0, 3).map(Number) ?? [];
  return red * 0.2126 + green * 0.7152 + blue * 0.0722 < 128;
}

function localDay(date: Date) {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate(), 12);
}

function addLocalDays(date: Date, days: number) {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() + days, 12);
}

function dateKey(date: Date) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function dateFromKey(value: string) {
  const [year, month, day] = value.split("-").map(Number);
  return new Date(year, month - 1, day, 12);
}

function finiteNonnegative(value: number) {
  return Number.isFinite(value) ? Math.max(0, value) : 0;
}
