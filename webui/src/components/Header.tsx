import { useState } from 'react';
import { useStore } from '../hooks/useStore.js';

interface HeaderProps {
  onStartDm: (targetUser: string) => void;
}

export function Header({ onStartDm }: HeaderProps) {
  const connected = useStore((s) => s.connected);
  const currentChannel = useStore((s) => s.currentChannel);
  const currentUser = useStore((s) => s.currentUser);
  const channels = useStore((s) => s.channels);
  const [showMembers, setShowMembers] = useState(false);

  // 当前频道信息
  const currentCh = channels.find((c) => c.name === currentChannel);
  const isChannel = currentCh?.kind === 'channel';
  const members = currentCh?.members ?? [];

  return (
    <header className="header">
      <div className="header-left">
        <span className="header-logo">GitIM</span>
        {currentChannel && (
          <span className="header-channel"># {currentChannel}</span>
        )}
      </div>
      <div className="header-right">
        {currentChannel && isChannel && (
          <div className="members-btn-wrapper">
            <button
              className="members-btn"
              onClick={() => setShowMembers((v) => !v)}
              title="成员列表"
            >
              👤 {members.length}
            </button>
            {showMembers && (
              <div className="members-dropdown">
                <div className="members-dropdown-title">成员 ({members.length})</div>
                {members.map((u) => (
                  <div key={u} className="members-dropdown-item">
                    <span className="members-dot" />
                    <span className="members-name">@ {u}{u === currentUser ? ' (我)' : ''}</span>
                    <button
                      className="members-dm-btn"
                      title={`发起私信: ${u}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        setShowMembers(false);
                        onStartDm(u);
                      }}
                    >
                      💬
                    </button>
                  </div>
                ))}
                {members.length === 0 && (
                  <div className="members-dropdown-item" style={{ color: 'var(--text-secondary)' }}>
                    暂无成员
                  </div>
                )}
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
