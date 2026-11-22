import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import CacheSection from './CacheSection.svelte';
import { AppStore } from '../store.svelte';
import { makeApi } from '../../test/fixtures';

function makeStore() {
  const api = makeApi();
  const store = new AppStore(api);
  store.cache = {
    downloads_bytes: 2048,
    downloads_files: 3,
    covers_bytes: 512,
    covers_files: 1,
  };
  return { api, store };
}

describe('CacheSection', () => {
  it('shows the download and cover sizes', () => {
    const { store } = makeStore();
    render(CacheSection, { props: { store } });

    expect(screen.getByText(/Downloads 2.0 KiB/)).toBeInTheDocument();
    expect(screen.getByText(/Covers 512 B/)).toBeInTheDocument();
  });

  it('asks for confirmation before it cleans', async () => {
    const { api, store } = makeStore();
    render(CacheSection, { props: { store } });

    await fireEvent.click(screen.getByRole('button', { name: 'Clean cache' }));
    expect(api.cleanCache).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Confirm clean?' })).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Confirm clean?' }));
    expect(api.cleanCache).toHaveBeenCalledWith('all');
  });

  it('cleans the selected scope', async () => {
    const { api, store } = makeStore();
    render(CacheSection, { props: { store } });

    const select = screen.getByLabelText('Cache scope');
    await fireEvent.change(select, { target: { value: 'covers' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Clean cache' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Confirm clean?' }));
    expect(api.cleanCache).toHaveBeenCalledWith('covers');
  });

  it('cancels the clean', async () => {
    const { api, store } = makeStore();
    render(CacheSection, { props: { store } });

    await fireEvent.click(screen.getByRole('button', { name: 'Clean cache' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.getByRole('button', { name: 'Clean cache' })).toBeInTheDocument();
    expect(api.cleanCache).not.toHaveBeenCalled();
  });
});
