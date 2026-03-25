import { useStore } from '../hooks/useStore.js';

interface SidebarProps {
  onChannelSelect: (name: string) => void;
}

export function Sidebar({ onChannelSelect }: SidebarProps) {
  const channels = useStore((s) => s.channels);
  const currentChannel = useStore((s) => s.currentChannel);

  const channelList = channels.filter((c) => c.kind === 'channel');
  const dmList = channels.filter((c) => c.kind === 'dm');

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
      {dmList.length > 0 && (
        <>
          <div className="sidebar-section-title">私信</div>
          {dmList.map((ch) => (
            <div
              key={ch.name}
              className={`sidebar-item ${ch.name === currentChannel ? 'active' : ''}`}
              onClick={() => onChannelSelect(ch.name)}
            >
              <span className="sidebar-item-name">@ {ch.name}</span>
              {ch.unreadCount > 0 && (
                <span className="unread-badge">{ch.unreadCount}</span>
              )}
            </div>
          ))}
        </>
      )}
    </aside>
  );
}
