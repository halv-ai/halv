import type { Config } from "tailwindcss";

export default {
  darkMode: "class",
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        surface: {
          base: "rgb(var(--surface-base) / <alpha-value>)",
          panel: "rgb(var(--surface-panel) / <alpha-value>)",
          raised: "rgb(var(--surface-raised) / <alpha-value>)",
          sunken: "rgb(var(--surface-sunken) / <alpha-value>)",
          glass: "rgb(var(--surface-glass) / <alpha-value>)",
          border: "rgb(var(--surface-border) / <alpha-value>)",
        },
        content: {
          primary: "rgb(var(--content-primary) / <alpha-value>)",
          secondary: "rgb(var(--content-secondary) / <alpha-value>)",
          muted: "rgb(var(--content-muted) / <alpha-value>)",
          inverse: "rgb(var(--content-inverse) / <alpha-value>)",
        },
        accent: {
          DEFAULT: "rgb(var(--accent) / <alpha-value>)",
          strong: "rgb(var(--accent-strong) / <alpha-value>)",
          muted: "rgb(var(--accent-muted) / <alpha-value>)",
          contrast: "rgb(var(--accent-contrast) / <alpha-value>)",
        },
        danger: {
          DEFAULT: "rgb(var(--danger) / <alpha-value>)",
          surface: "rgb(var(--danger-surface) / <alpha-value>)",
        },
        warning: {
          DEFAULT: "rgb(var(--warning) / <alpha-value>)",
          surface: "rgb(var(--warning-surface) / <alpha-value>)",
        },
        code: {
          bg: "rgb(var(--code-bg) / <alpha-value>)",
          header: "rgb(var(--code-header) / <alpha-value>)",
        },
      },
      fontFamily: {
        sans: ["Inter", "ui-sans-serif", "system-ui", "sans-serif"],
        mono: [
          "ui-monospace",
          "\"SF Mono\"",
          "\"JetBrains Mono\"",
          "\"Cascadia Mono\"",
          "\"Fira Code\"",
          "monospace",
        ],
      },
    },
  },
  plugins: [],
} satisfies Config;
