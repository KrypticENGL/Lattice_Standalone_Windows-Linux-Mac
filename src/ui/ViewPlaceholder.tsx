interface Props {
  title: string;
  description: string;
}

/** Stand-in for a view that has not been built yet. */
export function ViewPlaceholder({ title, description }: Props) {
  return (
    <div className="view-placeholder">
      <h2>{title}</h2>
      <p>{description}</p>
      <p className="muted">Not implemented yet.</p>
    </div>
  );
}
