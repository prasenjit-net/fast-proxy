/** A form-level error: a failure that is not tied to a single input. */
export default function FormError({ message }: { message: string | undefined }) {
  if (!message) return null;
  return (
    <p role="alert" className="rounded-lg bg-err-soft px-3 py-2 text-[0.84rem] text-err">
      {message}
    </p>
  );
}
