import type { ReactNode } from "react";

interface Props {
  title?: string;
  children: ReactNode;
  className?: string;
}

export function Panel({ title, children, className = "" }: Props) {
  return (
    <section className={`panel ${className}`}>
      {title && <div className="panel-header">{title}</div>}
      <div className="panel-body">{children}</div>
    </section>
  );
}
