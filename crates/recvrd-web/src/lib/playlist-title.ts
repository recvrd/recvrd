/** The title to use when no candidate is usable. */
export const PLAYLIST_FALLBACK_TITLE = 'Playlist';

/**
 * Pick a playlist title from the daemon detail name and the library name.
 *
 * The daemon returns the raw playlist ID as the name when the upstream page
 * has no header. Skip an empty name and skip the ID, so the UI never shows
 * the ID as a title.
 */
export function playlistTitle(
  id: string,
  ...names: Array<string | null | undefined>
): string {
  for (const name of names) {
    const title = name?.trim();
    if (title && title !== id) return title;
  }

  return PLAYLIST_FALLBACK_TITLE;
}
