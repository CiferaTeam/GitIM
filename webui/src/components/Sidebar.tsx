import { useState, useMemo } from 'react';
import { useStore } from '../hooks/useStore.js';

interface SidebarProps {
  onChannelSelect: (name: string) => void;
  onStartDm: (targetUser: string) => void;
}

export function Sidebar({ onChannelSelect, onStartDm }: SidebarProps) {
  const channels = useStore((s) => s.channels);
  const currentChannel = useStore((s) => s.currentChannel);
  const currentUser = useStore((s) => s.currentUser);
  const users = useStore((s) => s.users);

  const [dmSearchOpen, setDmSearchOpen] = useState(false);
  const [dmSearchFilter, setDmSearchFilter] = useState('');

  const channelList = channels.filter((c) => c.kind === 'channel');
  const dmListRaw = channels.filter((c) => c.kind === 'dm');

  // Sort DMs: self-DM pinned to top
  const dmList = useMemo(() => {
    return [...dmListRaw].sort((a, b) => {
      const aSelf = a.name.split('--').every((p) => p === currentUser);
      const bSelf = b.name.split('--').every((p) => p === currentUser);
      if (aSelf && !bSelf) return -1;
      if (!aSelf && bSelf) return 1;
      return 0;
    });
  }, [dmListRaw, currentUser]);

  // DM display name: show other user's handler; self-DM shows "username (我)"
  const dmDisplayName = (name: string) => {
    const parts = name.split('--');
    const isSelf = parts.every((p) => p === currentUser);
    if (isSelf) return `${currentUser} (我)`;
    const other = parts.find((p) => p !== currentUser) ?? name;
    return other;
  };

  // Filter users for search dropdown
  const filteredUsers = useMemo(() => {
    if (!dmSearchFilter) return users;
    const lower = dmSearchFilter.toLowerCase();
    return users.filter((u) => u.toLowerCase().includes(lower));
  }, [users, dmSearchFilter]);

  const handleSelectUser = (user: string) => {
    setDmSearchOpen(false);
    setDmSearchFilter('');
    onStartDm(user);
  };

  const handleSearchKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      setDmSearchOpen(false);
      setDmSearchFilter('');
    }
  };

  return (
    <aside className="sidebar">
      {channelList.length > 0 && (
        <>
          <div className="sidebar-section-title">频道</div>
          {channelList.map((ch) => (
            <div
              key={ch.name}
              className={`sidebar-item ${ch.name === currentChannel ? 'active' : ''}`}
              onClick={() => onChannelSelect(ch.name)}
            >
              <span className="sidebar-item-name"># {ch.name}</span>
              {ch.unreadCount > 0 && (
                <span className="unread-badge">{ch.unreadCount}</span>
              )}
            </div>
          ))}
        </>
      )}

      <div className="sidebar-section-title">
        私信
        {!dmSearchOpen && (
          <button
            className="sidebar-item dm-new-btn"
            onClick={() => setDmSearchOpen(true)}
          >
            + 发起新私信
          </button>
        )}
      </div>

      {dmSearchOpen && (
        <div className="dm-search">
          <input
            className="dm-search-input"
            placeholder="搜索用户..."
            autoFocus
            value={dmSearchFilter}
            onChange={(e) => setDmSearchFilter(e.target.value)}
            onKeyDown={handleSearchKeyDown}
          />
          <div className="dm-search-list">
            {filteredUsers.length > 0 ? (
              filteredUsers.map((u) => (
                <div
                  key={u}
                  className="dm-search-item"
                  onClick={() => handleSelectUser(u)}
                >
                  @ {u}
                </div>
              ))
            ) : (
              <div className="dm-search-empty">无匹配用户</div>
            )}
          </div>
        </div>
      )}

      {dmList.map((ch) => (
        <div
          key={ch.name}
          className={`sidebar-item ${ch.name === currentChannel ? 'active' : ''}`}
          onClick={() => onChannelSelect(ch.name)}
        >
          <span className="sidebar-item-name">@ {dmDisplayName(ch.name)}</span>
          {ch.unreadCount > 0 && (
            <span className="unread-badge">{ch.unreadCount}</span>
          )}
        </div>
      ))}
    </aside>
  );
}
