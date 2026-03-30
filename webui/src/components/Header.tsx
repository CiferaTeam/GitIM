import { useState } from 'react';
import { useStore } from '../hooks/useStore.js';

export function Header() {
  const connected = useStore((s) => s.connected);
  const currentChannel = useStore((s) => s.currentChannel);
  const currentUser = useStore((s) => s.currentUser);
  const users = useStore((s) => s.users);
  const [showMembers, setShowMembers] = useState(false);

  return (
    <header className="header">
      <div className="header-left">
        <span className="header-logo">GitIM</span>
        {currentChannel && (
          <span className="header-channel"># {currentChannel}</span>
        )}
      </div>
      <div className="header-right">
        {currentChannel && (
          <div className="members-btn-wrapper">
            <button
              className="members-btn"
              onClick={() => setShowMembers((v) => !v)}
              title="成员列表"
            >
              👤 {users.length}
            </button>
            {showMembers && (
              <div className="members-dropdown">
                <div className="members-dropdown-title">成员 ({users.length})</div>
                {users.map((u) => (
                  <div key={u} className="members-dropdown-item">
                    <span className="members-dot" />
                    @ {u}{u === currentUser ? ' (我)' : ''}
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
        <span className={`connection-dot ${connected ? 'online' : 'offline'}`} />
        {currentUser && <span className="header-user">@{currentUser}</span>}
      </div>
    </header>
  );
}
