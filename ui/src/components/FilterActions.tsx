// The Apply (and, once a filter is set, Clear) buttons at the end of a filter bar.
interface FilterActionsProps {
  /** A filter is applied, so there is something to clear. */
  filtered: boolean;
  onClear: () => void;
}

export default function FilterActions({ filtered, onClear }: FilterActionsProps) {
  return (
    <div className="flex gap-2">
      <button type="submit" className="btn btn-primary">
        Apply
      </button>
      {filtered ? (
        <button type="button" className="btn btn-ghost" onClick={onClear}>
          Clear
        </button>
      ) : null}
    </div>
  );
}
