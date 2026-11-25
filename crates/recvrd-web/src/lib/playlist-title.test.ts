import { describe, expect, it } from 'vitest';
import { PLAYLIST_FALLBACK_TITLE, playlistTitle } from './playlist-title';

const ID = 'VLPLEuTNuVCEhNwBcYRtEfw52PMGYPpyX1v4';

describe('playlistTitle', () => {
  it('uses the daemon detail name', () => {
    expect(playlistTitle(ID, 'Christmas Classics', 'Library name')).toBe(
      'Christmas Classics',
    );
  });

  it('skips a detail name that is the raw playlist ID', () => {
    expect(playlistTitle(ID, ID, 'Christmas Classics')).toBe('Christmas Classics');
  });

  it('skips an empty or whitespace detail name', () => {
    expect(playlistTitle(ID, '  ', 'Christmas Classics')).toBe('Christmas Classics');
  });

  it('trims the chosen name', () => {
    expect(playlistTitle(ID, ' Christmas Classics ')).toBe('Christmas Classics');
  });

  it('uses the fallback when every candidate is the ID or empty', () => {
    expect(playlistTitle(ID, ID, '', null, undefined)).toBe(PLAYLIST_FALLBACK_TITLE);
  });
});
