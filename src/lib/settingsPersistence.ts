/** Saves a setting and restores the visible value when the save fails. */
export async function saveWithRollback<T>(
  next: T,
  previous: T,
  save: (value: T) => Promise<unknown>,
  restore: (value: T) => void,
  report: (error: unknown) => void,
): Promise<boolean> {
  try {
    await save(next);
    return true;
  } catch (error) {
    restore(previous);
    report(error);
    return false;
  }
}
