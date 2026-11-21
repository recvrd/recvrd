import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import AuthBanner from './AuthBanner.svelte';
import ConnectionBanner from './ConnectionBanner.svelte';
import HelpOverlay from './HelpOverlay.svelte';

describe('AuthBanner', () => {
  it('gives remediation without secrets', () => {
    render(AuthBanner, {
      props: {
        auth: {
          state: 'missing',
          source: '/home/me/.config/recvrd/headers.txt',
          message: 'no cookie',
        },
      },
    });

    expect(screen.getByText('YouTube Music sign-in is missing.')).toBeInTheDocument();
    expect(screen.getByText('/home/me/.config/recvrd/headers.txt')).toBeInTheDocument();
    expect(screen.queryByText(/SAPISID=/)).not.toBeInTheDocument();
  });

  it('shows nothing for valid auth', () => {
    const { container } = render(AuthBanner, {
      props: {
        auth: { state: 'ok', source: '/tmp/headers.txt', message: null },
      },
    });
    expect(container.textContent?.trim()).toBe('');
  });

  it('explains an expired session without blocking search', () => {
    render(AuthBanner, {
      props: {
        auth: {
          state: 'expired',
          source: '/tmp/headers.txt',
          message:
            'The YouTube Music session expired. Update headers.txt with a fresh Cookie and restart the daemon. Search and playback still work.',
        },
      },
    });

    expect(screen.getByText('YouTube Music session expired.')).toBeInTheDocument();
    expect(screen.getByText('Library and playlist actions need a new cookie.')).toBeInTheDocument();
    expect(screen.getByText(/Search and playback still work/)).toBeInTheDocument();
    expect(screen.getByText('/tmp/headers.txt')).toBeInTheDocument();
  });
});

describe('ConnectionBanner', () => {
  it('shows reconnect state and calls retry', async () => {
    const onretry = vi.fn();
    render(ConnectionBanner, {
      props: { connection: 'reconnecting', error: null, onretry },
    });
    expect(screen.getByText('Connection lost. Reconnecting…')).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Retry now' }));
    expect(onretry).toHaveBeenCalledOnce();
  });

  it('shows the offline error', () => {
    render(ConnectionBanner, {
      props: {
        connection: 'offline',
        error: 'Daemon is unreachable. Is recvrd running?',
        onretry: () => {},
      },
    });
    expect(screen.getByText('The daemon is offline.')).toBeInTheDocument();
    expect(
      screen.getByText('Daemon is unreachable. Is recvrd running?'),
    ).toBeInTheDocument();
  });
});

describe('HelpOverlay', () => {
  it('lists keys and closes', async () => {
    const onclose = vi.fn();
    render(HelpOverlay, { props: { open: true, onclose } });
    expect(screen.getByRole('dialog', { name: 'Keyboard and mouse help' })).toBeInTheDocument();
    expect(screen.getByText('Half page down')).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Close help' }));
    expect(onclose).toHaveBeenCalledOnce();
  });
});
