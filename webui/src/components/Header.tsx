import { useStore } from '../hooks/useStore.js';

export function Header() {
  const connected = useStore((s) => s.connected);
  const currentChannel = useStore((s) => s.currentChannel);
  const currentUser = useStore((s) => s.currentUser);

  return (
    <header className="header">
      <div className="header-left">
        <span className="header-logo">GitIM</span>
        {currentChannel && (
          <span className="header-channel"># {currentChannel}</span>
        )}
      </div>
      <div className="header-right">
        <span className={`connection-dot ${connected ? 'online' : 'offline'}`} />
        {currentUser && <span className="header-user">@{currentUser}</span>}
      </div>
    </header>
  );
}
