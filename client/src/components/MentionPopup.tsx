import { useEffect, useState, useCallback, useRef } from 'react';

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

  // 用 ref 追踪最新值，避免 handleKeyDown 频繁重建
  const filteredRef = useRef(filtered);
  filteredRef.current = filtered;
  const activeIndexRef = useRef(activeIndex);
  activeIndexRef.current = activeIndex;
  const onSelectRef = useRef(onSelect);
  onSelectRef.current = onSelect;
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  // 过滤结果变化时重置选中
  useEffect(() => {
    setActiveIndex(0);
  }, [filter]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      const f = filteredRef.current;
      const idx = activeIndexRef.current;
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setActiveIndex((i) => (i + 1) % f.length);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setActiveIndex((i) => (i - 1 + f.length) % f.length);
      } else if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        if (f.length > 0) {
          onSelectRef.current(f[idx]);
        }
      } else if (e.key === 'Escape') {
        e.preventDefault();
        onCloseRef.current();
      }
    },
    [],
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
