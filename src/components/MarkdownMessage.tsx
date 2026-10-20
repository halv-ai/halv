import { externalLinkUrl, handleMarkdownLinkClick } from "../externalLinks.ts";
import { lazy, memo, Suspense } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

const CodeBlock = lazy(() =>
  import("./CodeBlock").then((module) => ({ default: module.CodeBlock })),
);

type MarkdownMessageProps = {
  content: string;
  className?: string;
};

const components: Components = {
  a({ href, children, ...props }) {
    return (
      <a
        href={href}
        onClick={(event) => handleMarkdownLinkClick(event, href)}
        rel={externalLinkUrl(href) ? "noreferrer" : undefined}
        {...props}
      >
        {children}
      </a>
    );
  },
  code({ className, children, ...props }) {
    const code = String(children).replace(/\n$/, "");
    const match = /language-([\w-]+)/.exec(className ?? "");
    const language = match?.[1] ?? "";
    const isBlock = Boolean(language) || code.includes("\n");

    if (!isBlock) {
      return (
        <code
          className="rounded bg-code-header px-1 py-0.5 font-mono text-[0.9em] text-content-primary"
          {...props}
        >
          {children}
        </code>
      );
    }

    return (
      <Suspense fallback={<PlainCodeBlock code={code} />}>
        <CodeBlock code={code} language={language} />
      </Suspense>
    );
  },
};

export const MarkdownMessage = memo(function MarkdownMessage({
  className = "",
  content,
}: MarkdownMessageProps) {
  return (
    <div className={`markdown-body ${className}`}>
      <ReactMarkdown components={components} remarkPlugins={[remarkGfm]}>
        {content}
      </ReactMarkdown>
    </div>
  );
});

function PlainCodeBlock({ code }: { code: string }) {
  return (
    <pre className="my-3 overflow-x-auto rounded-md border border-surface-border/30 bg-code-bg p-3 font-mono text-sm leading-6">
      <code className="block bg-transparent">{code}</code>
    </pre>
  );
}
