import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Components } from "react-markdown";
import { cn } from "@/lib/utils";

const components: Components = {
  h1: ({ children, className, ...props }) => (
    <h3 className={cn("text-base font-semibold mt-3 mb-1 first:mt-0 text-foreground", className)} {...props}>
      {children}
    </h3>
  ),
  h2: ({ children, className, ...props }) => (
    <h4 className={cn("text-sm font-semibold mt-3 mb-1 first:mt-0 text-foreground", className)} {...props}>
      {children}
    </h4>
  ),
  h3: ({ children, className, ...props }) => (
    <h5 className={cn("text-sm font-semibold mt-2 mb-1 text-foreground", className)} {...props}>
      {children}
    </h5>
  ),
  p: ({ children, className, ...props }) => (
    <p className={cn("mb-2 last:mb-0 leading-relaxed text-foreground", className)} {...props}>
      {children}
    </p>
  ),
  ul: ({ children, className, ...props }) => (
    <ul className={cn("list-disc pl-4 mb-2 space-y-0.5 text-foreground", className)} {...props}>
      {children}
    </ul>
  ),
  ol: ({ children, className, ...props }) => (
    <ol className={cn("list-decimal pl-4 mb-2 space-y-0.5 text-foreground", className)} {...props}>
      {children}
    </ol>
  ),
  li: ({ children, className, ...props }) => (
    <li className={cn("leading-relaxed", className)} {...props}>
      {children}
    </li>
  ),
  strong: ({ children, className, ...props }) => (
    <strong className={cn("font-semibold text-foreground", className)} {...props}>
      {children}
    </strong>
  ),
  em: ({ children, className, ...props }) => (
    <em className={cn("italic", className)} {...props}>
      {children}
    </em>
  ),
  a: ({ href, children, className, ...props }) => (
    <a
      href={href}
      className={cn("text-primary underline underline-offset-2 hover:opacity-90", className)}
      target="_blank"
      rel="noopener noreferrer"
      {...props}
    >
      {children}
    </a>
  ),
  blockquote: ({ children, className, ...props }) => (
    <blockquote
      className={cn(
        "border-l-2 border-primary/40 pl-3 my-2 text-muted-foreground italic text-sm",
        className
      )}
      {...props}
    >
      {children}
    </blockquote>
  ),
  hr: ({ className, ...props }) => <hr className={cn("my-3 border-border", className)} {...props} />,
  code: ({ className, children, ...props }) => {
    const isBlock = Boolean(className?.includes("language-"));
    if (!isBlock) {
      return (
        <code
          className="bg-muted px-1 py-0.5 rounded text-[0.9em] font-mono text-foreground"
          {...props}
        >
          {children}
        </code>
      );
    }
    return (
      <code className={cn("font-mono text-xs text-foreground", className)} {...props}>
        {children}
      </code>
    );
  },
  pre: ({ children, className, ...props }) => (
    <pre
      className={cn(
        "bg-muted/80 rounded-md p-2 overflow-x-auto mb-2 border border-border/50 [&>code]:bg-transparent [&>code]:p-0",
        className
      )}
      {...props}
    >
      {children}
    </pre>
  ),
  table: ({ children, className, ...props }) => (
    <div className="overflow-x-auto my-2 rounded-md border border-border/60">
      <table className={cn("w-full text-xs border-collapse text-foreground", className)} {...props}>
        {children}
      </table>
    </div>
  ),
  thead: ({ children, className, ...props }) => (
    <thead className={cn("bg-muted/50", className)} {...props}>
      {children}
    </thead>
  ),
  th: ({ children, className, ...props }) => (
    <th
      className={cn("border border-border/60 px-2 py-1.5 text-left font-semibold", className)}
      {...props}
    >
      {children}
    </th>
  ),
  td: ({ children, className, ...props }) => (
    <td className={cn("border border-border/60 px-2 py-1.5 align-top", className)} {...props}>
      {children}
    </td>
  ),
  tr: ({ children, className, ...props }) => (
    <tr className={cn("border-border/60", className)} {...props}>
      {children}
    </tr>
  ),
};

interface Props {
  children: string;
  className?: string;
}

export function SummaryMarkdown({ children, className }: Props) {
  return (
    <div className={cn("text-sm max-h-64 overflow-y-auto", className)}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {children}
      </ReactMarkdown>
    </div>
  );
}
