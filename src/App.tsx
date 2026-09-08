import { useEffect, useMemo, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import "./App.css";

type Mode = "similar" | "screenshots" | "time";
type TimeBasis = "captured" | "created" | "modified";

type ImageItem = {
  id: string;
  path: string;
  name: string;
  bytes: number;
  width: number;
  height: number;
  date: string;
  dateSource: string;
  groupId: number | null;
  groupSize: number;
};

type ScanResponse = {
  items: ImageItem[];
  totalScanned: number;
};

type UndoEntry = {
  item: ImageItem;
  backupPath: string;
  index: number;
  viewKey: string;
};

const modes: { id: Mode; label: string; icon: string; description: string }[] = [
  { id: "similar", label: "相似图片", icon: "◌", description: "找出重复拍摄、裁剪或压缩后的近似照片" },
  { id: "screenshots", label: "手机截图", icon: "▣", description: "按文件名和手机屏幕比例筛选截图" },
  { id: "time", label: "按时间", icon: "◷", description: "按拍摄、创建或修改日期筛选" },
];

const timeBases: { id: TimeBasis; label: string }[] = [
  { id: "captured", label: "拍摄时间（EXIF）" },
  { id: "created", label: "文件创建时间" },
  { id: "modified", label: "文件修改时间" },
];

function formatBytes(bytes: number) {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function App() {
  const [mode, setMode] = useState<Mode>("similar");
  const [timeBasis, setTimeBasis] = useState<TimeBasis>("captured");
  const [startDate, setStartDate] = useState("");
  const [endDate, setEndDate] = useState("");
  const [folder, setFolder] = useState<string | null>(null);
  const [items, setItems] = useState<ImageItem[]>([]);
  const [activeIndex, setActiveIndex] = useState(0);
  const [scannedCount, setScannedCount] = useState(0);
  const [isScanning, setIsScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [undoStack, setUndoStack] = useState<UndoEntry[]>([]);

  const activeItem = items[activeIndex] ?? null;
  const activeMode = modes.find((item) => item.id === mode) ?? modes[0];
  const similarGroups = useMemo(() => {
    const groups = new Map<number, number[]>();
    items.forEach((item, index) => {
      if (item.groupId !== null) {
        groups.set(item.groupId, [...(groups.get(item.groupId) ?? []), index]);
      }
    });
    return [...groups.values()];
  }, [items]);
  const viewKey = [folder, mode, timeBasis, startDate, endDate].join("\u0000");

  async function scan(
    nextFolder = folder,
    nextMode = mode,
    nextTimeBasis = timeBasis,
    nextStartDate = startDate,
    nextEndDate = endDate,
  ) {
    if (!nextFolder) return;
    setIsScanning(true);
    setError(null);
    setNotice(null);
    try {
      const result = await invoke<ScanResponse>("scan_folder", {
        options: {
          rootPath: nextFolder,
          mode: nextMode,
          timeBasis: nextTimeBasis,
          startDate: nextStartDate || null,
          endDate: nextEndDate || null,
        },
      });
      setItems(result.items);
      setScannedCount(result.totalScanned);
      setActiveIndex(0);
      if (result.items.length === 0) {
        setNotice(nextMode === "similar" ? "没有找到足够相似的图片。" : "当前条件下没有找到图片。");
      }
    } catch (scanError) {
      setError(String(scanError));
    } finally {
      setIsScanning(false);
    }
  }

  async function chooseFolder() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected !== "string") return;
    setFolder(selected);
    await scan(selected);
  }

  async function removeActive() {
    if (!activeItem || isScanning) return;
    try {
      const backupPath = await invoke<string>("move_to_trash", { path: activeItem.path });
      setUndoStack((current) => [...current, { item: activeItem, backupPath, index: activeIndex, viewKey }]);
      setItems((current) => current.filter((item) => item.id !== activeItem.id));
      setActiveIndex((current) => Math.min(current, Math.max(0, items.length - 2)));
      setNotice(`已移入回收站：${activeItem.name}`);
    } catch (deleteError) {
      setError(String(deleteError));
    }
  }

  async function undoLastDelete() {
    if (isScanning || undoStack.length === 0) return;
    const entry = undoStack[undoStack.length - 1];
    try {
      await invoke("restore_from_undo", { backupPath: entry.backupPath, originalPath: entry.item.path });
      setUndoStack((current) => current.slice(0, -1));
      if (entry.viewKey === viewKey) {
        setItems((current) => {
          const next = [...current];
          next.splice(Math.min(entry.index, next.length), 0, entry.item);
          return next;
        });
        setActiveIndex(Math.min(entry.index, items.length));
      } else if (folder) {
        await scan();
      }
      setNotice(`已撤销删除：${entry.item.name}`);
    } catch (undoError) {
      setError(String(undoError));
    }
  }

  function moveWithinGroup(direction: -1 | 1) {
    const group = similarGroups.find((candidate) => candidate.includes(activeIndex));
    if (!group) return;
    const position = group.indexOf(activeIndex);
    const nextIndex = group[position + direction];
    if (nextIndex !== undefined) setActiveIndex(nextIndex);
  }

  function moveGroup(direction: -1 | 1) {
    const currentGroup = similarGroups.findIndex((candidate) => candidate.includes(activeIndex));
    const nextGroup = similarGroups[currentGroup + direction];
    if (nextGroup?.[0] !== undefined) setActiveIndex(nextGroup[0]);
  }

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      const target = event.target as HTMLElement | null;
      if (target?.tagName === "INPUT" || target?.tagName === "SELECT" || target?.tagName === "TEXTAREA") return;
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "z") {
        if (undoStack.length === 0) return;
        event.preventDefault();
        void undoLastDelete();
        return;
      }
      const inSimilarGroup = mode === "similar" && activeItem !== null && activeItem.groupId !== null;
      if (event.key === "ArrowUp" || event.key === "ArrowDown") {
        if (!inSimilarGroup) return;
        event.preventDefault();
        moveGroup(event.key === "ArrowUp" ? -1 : 1);
      } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        event.preventDefault();
        if (inSimilarGroup) {
          moveWithinGroup(event.key === "ArrowLeft" ? -1 : 1);
        } else if (items.length > 0) {
          setActiveIndex((current) => event.key === "ArrowLeft" ? Math.max(0, current - 1) : Math.min(items.length - 1, current + 1));
        }
      } else if (event.key === "Delete" || event.key === "Backspace" || event.key.toLowerCase() === "d") {
        event.preventDefault();
        void removeActive();
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [items, activeItem, isScanning, mode, similarGroups, undoStack, viewKey, folder, timeBasis, startDate, endDate]);

  function changeMode(nextMode: Mode) {
    setMode(nextMode);
    if (folder) void scan(folder, nextMode);
  }

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">✦</div>
          <div>
            <h1>Picture Cleaner</h1>
            <span>本地图片整理工具</span>
          </div>
        </div>

        <nav className="mode-list" aria-label="清理模式">
          <div className="nav-caption">清理模式</div>
          {modes.map((item) => (
            <button className={`mode-button ${item.id === mode ? "active" : ""}`} key={item.id} onClick={() => changeMode(item.id)}>
              <span className="mode-icon">{item.icon}</span>
              <span>
                <strong>{item.label}</strong>
                <small>{item.description}</small>
              </span>
            </button>
          ))}
        </nav>

        <div className="sidebar-footnote">
          {mode === "similar" && <><span className="key-hint">↑</span><span className="key-hint">↓</span> 切换分组<br /></>}
          <span className="key-hint">←</span><span className="key-hint">→</span> 切换图片
          <br />
          <span className="key-hint">D</span><span className="key-hint">⌫</span> 移入回收站
          <br />
          <span className="key-hint">⌘Z</span><span className="key-hint">Ctrl Z</span> 撤销删除
        </div>
      </aside>

      <section className="workspace">
        <header className="topbar">
          <div>
            <p className="eyebrow">{activeMode.label}</p>
            <h2>{folder ? folder.split(/[\\/]/).pop() : "先选择一个图片文件夹"}</h2>
          </div>
          <button className="folder-button" onClick={() => void chooseFolder()}>
            <span>＋</span> 选择文件夹
          </button>
        </header>

        <div className="toolbar">
          {mode === "time" && (
            <>
              <label>
                时间依据
                <select value={timeBasis} onChange={(event) => setTimeBasis(event.target.value as TimeBasis)}>
                  {timeBases.map((basis) => <option value={basis.id} key={basis.id}>{basis.label}</option>)}
                </select>
              </label>
              <label>
                从
                <input type="date" value={startDate} onChange={(event) => setStartDate(event.target.value)} />
              </label>
              <label>
                到
                <input type="date" value={endDate} onChange={(event) => setEndDate(event.target.value)} />
              </label>
            </>
          )}
          <button className="scan-button" disabled={!folder || isScanning} onClick={() => void scan()}>
            {isScanning ? "扫描中…" : "重新扫描"}
          </button>
          <span className="scan-summary">{scannedCount ? `已扫描 ${scannedCount} 张` : "支持 JPG、PNG、WebP 等常见格式"}</span>
        </div>

        {error && <div className="message error">{error}</div>}
        {notice && <div className="message">{notice}</div>}

        {!folder ? (
          <div className="empty-state welcome-state">
            <div className="empty-orbit">✦</div>
            <h3>从选择一个文件夹开始</h3>
            <p>图片只在本机处理，删除时会移入系统回收站，可随时恢复。</p>
            <button className="primary-button" onClick={() => void chooseFolder()}>选择图片文件夹</button>
          </div>
        ) : isScanning ? (
          <div className="empty-state">
            <div className="spinner" />
            <h3>正在分析图片…</h3>
            <p>相似图片模式会计算图片的视觉指纹，请稍候。</p>
          </div>
        ) : activeItem ? (
          <div className="review-layout">
            <div className="preview-card">
              <div className="preview-stage">
                <img src={convertFileSrc(activeItem.path)} alt={activeItem.name} draggable={false} />
                <button className="nav-arrow left" disabled={activeIndex === 0} onClick={() => setActiveIndex((current) => Math.max(0, current - 1))}>‹</button>
                <button className="nav-arrow right" disabled={activeIndex === items.length - 1} onClick={() => setActiveIndex((current) => Math.min(items.length - 1, current + 1))}>›</button>
              </div>
              <div className="preview-footer">
                <div>
                  <strong>{activeItem.name}</strong>
                  <span>{activeItem.width} × {activeItem.height} · {formatBytes(activeItem.bytes)}</span>
                </div>
                <button className="delete-button" onClick={() => void removeActive()}>移入回收站</button>
              </div>
            </div>

            <aside className="details-panel">
              <div className="progress-label"><span>当前图片</span><strong>{activeIndex + 1} / {items.length}</strong></div>
              <div className="progress-track"><span style={{ width: `${((activeIndex + 1) / items.length) * 100}%` }} /></div>
              <div className="detail-block">
                <span className="detail-label">日期</span>
                <strong>{activeItem.date}</strong>
                <small>{activeItem.dateSource === "captured" ? "来自 EXIF 拍摄时间" : `来自${activeItem.dateSource === "created" ? "文件创建时间" : "文件修改时间"}`}</small>
              </div>
              {activeItem.groupId !== null && <div className="similar-badge">相似组 {activeItem.groupId + 1} · 共 {activeItem.groupSize} 张</div>}
              <div className="detail-block shortcut-block">
                <span className="detail-label">快捷键</span>
                {mode === "similar" && <div><kbd>↑</kbd><kbd>↓</kbd><span>切换分组</span></div>}
                <div><kbd>←</kbd><kbd>→</kbd><span>切换图片</span></div>
                <div><kbd>Delete</kbd><kbd>D</kbd><kbd>⌫</kbd><span>移入回收站</span></div>
                <div><kbd>⌘Z</kbd><kbd>Ctrl Z</kbd><span>撤销删除</span></div>
              </div>
            </aside>
          </div>
        ) : (
          <div className="empty-state">
            <div className="empty-icon">✓</div>
            <h3>这里很干净</h3>
            <p>没有需要处理的图片。</p>
          </div>
        )}

        {items.length > 1 && (
          <div className="thumbnail-strip" aria-label="图片列表">
            {items.map((item, index) => (
              <button className={`thumbnail ${index === activeIndex ? "selected" : ""}`} key={item.id} onClick={() => setActiveIndex(index)} title={item.name}>
                <img src={convertFileSrc(item.path)} alt="" draggable={false} />
                {item.groupId !== null && <span>{item.groupId + 1}</span>}
              </button>
            ))}
          </div>
        )}
        {mode === "similar" && similarGroups.length > 0 && <p className="footer-note">已找到 {similarGroups.length} 组相似图片，↑↓ 切换分组，←→ 浏览组内图片。</p>}
      </section>
    </main>
  );
}

export default App;
