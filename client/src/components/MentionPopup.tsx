import { useEffect, useState, useCallback } from 'react';

interface MentionPopupProps {
  users: string[];
  filter: string; // @后面已输入的文字
  onSelect: (handle: string) => void;
  onClose: () => void;
}

export function MentionPopup({ users, filter, onSelect, onClose }: MentionPopupProps) {
  const [activeIndex, setActiveIndex] = useState(0);

  const filtered = users.filter((u) =>
    u.toLowerCase().includes(filter.toLowerCase()),
  );

  // 过滤结果变化时重置选中
  useEffect(() => {
    setActiveIndex(0);
  }, [filter]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setActiveIndex((i) => (i + 1) % filtered.length);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setActiveIndex((i) => (i - 1 + filtered.length) % filtered.length);
      } else if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        if (filtered.length > 0) {
          onSelect(filtered[activeIndex]);
        }
      } else if (e.key === 'Escape') {
        e.preventDefault();
        onClose();
      }
    },
    [filtered, activeIndex, onSelect, onClose],
  );

  useEffect(() => {
    document.addEventListener('keydown', handleKeyDown, true);
    return () => document.removeEventListener('keydown', handleKeyDown, true);
  }, [handleKeyDown]);

  if (filtered.length === 0) return null;

  return (
    <div className="mention-popup">
      {filtered.map((user, i) => (
        <div
          key={user}
          className={`mention-item ${i === activeIndex ? 'active' : ''}`}
          onMouseEnter={() => setActiveIndex(i)}
          onClick={() => onSelect(user)}
        >
          @{user}
        </div>
      ))}
    </div>
  );
}
