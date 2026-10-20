import { mountDesktop, type Direction } from './desktop';

const paths: Record<string, string> = {
  lock: 'M7 10V7a5 5 0 0 1 10 0v3M5 10h14v11H5zM12 14v3',
  file: 'M14 2H6v20h14V8zM14 2v6h6M9 13h8M9 17h6',
  folder: 'M3 6h7l2 3h9v11H3z',
  upload: 'M12 16V3m-5 5 5-5 5 5M4 15v6h16v-6',
  download: 'M12 3v13m-5-5 5 5 5-5M4 15v6h16v-6',
  search: 'M21 21l-5-5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0',
  check: 'm5 12 4 4L20 5',
  trash: 'M3 6h18M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7M14 10v7',
};
const icon = (name: string) => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${paths[name]}"/></svg>`;
const files = [{ name: '旅行照片.jpg', size: '4.8 MB', kind: 'JPG', color: 'peach' }, { name: '一些小秘密.txt', size: '1.2 KB', kind: 'TXT', color: 'sage' }, { name: '证件备份.pdf', size: '2.1 MB', kind: 'PDF', color: 'lavender' }];
const steps = [
  { label: '解锁', status: '点击「解锁保险库」开始体验' },
  { label: '存入', status: '把桌面文件拖入 GUI' },
  { label: '查看', status: '点击文件查看详情，或拖回桌面' },
  { label: '取出', status: '把库内文件拖到右侧桌面' },
  { label: '锁定', status: '已锁定 · 点击保险库重新打开' },
];
function get<T extends HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (!element) throw new Error(`Missing demo element: ${id}`);
  return element as T;
}
const screen = get('app-screen'), play = get<HTMLButtonElement>('play'), progress = get<HTMLInputElement>('progress');
const duration = 6000, total = duration * steps.length;
let elapsed = 0, current = -1, playing = false, speed = 1, previous = 0, frame = 0;
let selected = 0, query = '';
let manual = false;
const importedIds = new Set<number>(), exportedIds = new Set<number>();
let signature = '';
const action = (label: string, value: string, highlight = false) => `<button type="button" data-action="${value}" class="app-button ${highlight ? 'spotlight' : ''}">${label}</button>`;

function login(completed: boolean): string {
  return `<div class="login-area"><div class="login-card">
    <div class="login-symbol">${icon(completed ? 'check' : 'lock')}</div>
    <h4>${completed ? '你的私人空间，已锁好。' : '欢迎回到你的文件库'}</h4>
    <p>${completed ? '这一轮演示结束，随时可以重新体验。' : '重要的东西，都在这里。'}</p>
    <div class="login-tabs"><span>打开保险库</span><button disabled title="演示暂不包含创建流程">创建新保险库</button><button disabled title="演示暂不包含恢复流程">整库恢复</button></div>
    <label>保险库目录<input readonly value="/我的移动硬盘/我的随身库" /></label>
    <label>密码<input readonly type="password" value="example-only" autocomplete="off" /></label>
    ${action(completed ? '↺ 重新体验' : '解锁保险库 →', completed ? 'restart' : 'next', true)}
    <small>这是示例界面，请勿输入真实密码。</small>
  </div></div>`;
}
function rows(): string {
  return files.map((file, index) => importedIds.has(index) && file.name.includes(query) ? `<button type="button" class="file-row" data-file="${index}" aria-pressed="${selected === index}" aria-label="${file.name}，拖到桌面导出"><span class="file-symbol ${file.color}">${icon('file')}</span><span class="file-name">${file.name}<small>已加密 · 示例文件</small></span><span class="file-size">${file.size}</span><span class="file-check">${selected === index ? '✓' : ''}</span></button>` : '').join('') || '<p class="no-results">没有匹配的示例文件</p>';
}
function details(): string {
  const file = files[selected];
  return `<div class="detail-icon ${file.color}">${icon('file')}</div><h4>${file.name}</h4><p>${file.kind} 文件 · ${file.size}</p><div class="detail-metadata"><span>存储状态</span><b>已加密（示例）</b><span>所在位置</span><b>根目录</b></div>`;
}
function manager(): string {
  const imported = importedIds.size > 0, count = importedIds.size;
  return `<div class="file-manager">
    <aside class="app-sidebar"><p>我的随身库</p><span class="sidebar-active">${icon('folder')}全部文件 <b>${count}</b></span><button disabled title="本轮演示聚焦文件导入与导出">${icon('trash')}回收站</button><small><i></i> 可写会话</small></aside>
    <div class="file-browser" tabindex="0" role="region" aria-label="保险库文件区域"><div class="browser-path">${icon('folder')} 根目录 <span>/ 我的随身库</span></div><label class="search-field">${icon('search')}<input id="file-search" aria-label="搜索示例文件" placeholder="搜索保险库中的全部名称…" ${imported ? '' : 'disabled'} /></label><div class="list-caption"><span>名称</span><span>${count} 个项目</span></div>
    <div id="file-list">${imported ? rows() : `<div class="empty-files">${icon('upload')}<h4>把桌面文件，拖到这里。</h4><p>松手后，文件会加密导入</p><div class="sample-stack"><span>JPG</span><span>TXT</span><span>PDF</span></div></div>`}</div></div>
    <aside class="app-actions"><div id="file-details">${imported ? details() : `<div class="detail-icon">${icon('upload')}</div><h4>导入与新建</h4><p>支持从桌面直接拖入</p>`}</div><div class="action-divider"></div><h5>${imported ? '导入、导出与替换' : '导入与新建'}<span>⌄</span></h5><label>本地文件路径<input readonly value="${imported ? '/桌面/' : '/桌面/示例文件/'}" /></label>${imported ? action(`${icon('download')} 导出`, 'export') : action(`${icon('upload')} 导入`, 'import')}<p class="action-hint">${imported ? '也可拖动文件行到桌面' : '也可从桌面直接拖入'}</p><div class="action-divider"></div><p class="drawer-label">移动与回收 <span>›</span></p><p class="drawer-label">保险库维护 <span>›</span></p><p class="drawer-label">密码与恢复密钥 <span>›</span></p></aside>
  </div>`;
}
function render(): void {
  const locked = current === 0 || current === 4, step = steps[current];
  screen.innerHTML = `<div class="app-header"><span class="app-brand">兄弟懂我的保险箱</span><span class="app-language">简体中文⌄</span><span class="lock-status ${locked ? '' : 'unlocked'}">${icon('lock')}${locked ? '已锁定' : '已解锁'}</span><button class="app-button ${current === 3 ? 'spotlight' : ''}" data-action="lock" ${locked ? 'disabled' : ''}>锁定</button></div>${locked ? login(current === 4) : manager()}<div class="app-status"><span>实验版本</span><p>${step.status}</p></div>`;
  const search = screen.querySelector<HTMLInputElement>('#file-search');
  if (search) search.value = query;
  get('steps').innerHTML = steps.map((item, index) => {
    let marker = '';
    if (index < current) marker = '✓';
    else if (index === current) marker = '←';
    return `<button class="step" type="button" data-step="${index}" ${index === current ? 'aria-current="step"' : ''}><span>0${index + 1}</span>${item.label}<span>${marker}</span></button>`;
  }).join('');
  screen.dataset.step = String(current);
  get('vault-stage').classList.toggle('is-open', !locked);
  get('vault-stage').classList.toggle('is-stowed', current === 4);
  get('vault-summary').textContent = `${importedIds.size} 个文件 · ${locked ? '已锁定' : '已解锁'}`;
  get('vault-folder').setAttribute('aria-label', `${locked ? '打开' : '查看'}我的随身库，${importedIds.size} 个文件，${locked ? '已锁定' : '已解锁'}`);
  // Keep the retracted interface out of the keyboard and accessibility order.
  screen.closest<HTMLElement>('.vault-projection')!.inert = current === 4;
  desktop.sync(importedIds, exportedIds);
  desktop.note(step.status);
}
function update(): void {
  const next = Math.min(Math.floor(elapsed / duration), steps.length - 1);
  const phase = elapsed >= total ? 1 : (elapsed % duration) / duration;
  if (!manual) {
    importedIds.clear(); exportedIds.clear();
    if (next >= 2 || next === 1 && phase >= .42) files.forEach((_, index) => importedIds.add(index));
    if (next >= 4 || next === 3 && phase >= .42) exportedIds.add(0);
  }
  if (next !== current) { query = ''; if (!manual) selected = 0; }
  if (!importedIds.has(selected)) selected = importedIds.values().next().value ?? 0;
  const state = `${next}:${[...importedIds]}:${[...exportedIds]}:${selected}`;
  if (state !== signature) {
    current = next; signature = state;
    render();
  }
  const percent = elapsed / total * 100;
  progress.value = percent.toFixed(1);
  progress.style.setProperty('--progress', `${percent}%`);
  progress.setAttribute('aria-valuetext', `${Math.round(percent)}%，${steps[current].label}`);
  get('progress-label').textContent = `0${current + 1} / 05 · ${Math.round(percent)}%`;
  const direction: Direction | null = current === 1 ? 'import' : current === 3 ? 'export' : null;
  desktop.animate(!manual && direction && phase < .42 ? direction : null, phase / .42);
}
function tick(now: number): void {
  elapsed = Math.min(total, elapsed + Math.max(0, now - previous) * speed);
  previous = now;
  update();
  if (elapsed >= total) setPlaying(false);
  else if (playing) frame = requestAnimationFrame(tick);
}
function setPlaying(value: boolean): void {
  cancelAnimationFrame(frame);
  playing = value;
  play.textContent = '▶ 慢速播放';
  if (value) play.textContent = 'Ⅱ 暂停演示';
  else if (elapsed >= total) play.textContent = '↺ 再看一遍';
  play.setAttribute('aria-pressed', String(value));
  if (value) { previous = performance.now(); frame = requestAnimationFrame(tick); }
}
function seek(value: number): void {
  setPlaying(false); desktop.cancel(); manual = false; signature = ''; selected = 0; query = '';
  elapsed = Math.max(0, Math.min(total, value)); update(); setPlaying(false);
}
function allowed(kind: Direction, index: number): boolean {
  return current > 0 && current < 4 && (kind === 'import' ? !importedIds.has(index) : importedIds.has(index));
}
function transfer(kind: Direction, index: number): void {
  if (!allowed(kind, index)) {
    desktop.note(current === 0 || current === 4 ? '请先解锁保险库，再拖入文件。' : '这个文件已在库内，桌面源文件会保留。'); return;
  }
  const active = document.activeElement;
  const restoreFocus = screen.contains(active);
  setPlaying(false); desktop.cancel(); manual = true; selected = index;
  if (kind === 'export' && exportedIds.has(index)) desktop.activity('export');
  (kind === 'import' ? importedIds : exportedIds).add(index);
  elapsed = (kind === 'import' ? 2 : 3.99) * duration;
  update(); setPlaying(false);
  if (restoreFocus && !active?.isConnected) {
    const target = screen.querySelector<HTMLElement>(`.file-row[data-file="${selected}"]`) ?? screen.querySelector<HTMLElement>('.file-browser');
    target?.focus({ preventScroll: true });
  }
  desktop.note(`${files[index].name} ${kind === 'import' ? '已存入保险库，桌面源文件保留。' : '已导出到模拟桌面，库内文件保留。'}`);
}
const desktop = mountDesktop(files, icon, transfer, allowed);
get('vault-folder').addEventListener('click', () => {
  if (current === 4) {
    setPlaying(false); desktop.cancel(); manual = true;
    elapsed = 0; signature = ''; update(); setPlaying(false);
  }
  screen.querySelector<HTMLElement>(current === 0 ? '[data-action="next"]' : '#file-search')?.focus({ preventScroll: true });
});
get('desktop').addEventListener('manual-drag', () => setPlaying(false));
play.addEventListener('click', () => { if (elapsed >= total || manual) seek(0); setPlaying(!playing); });
get('restart').addEventListener('click', () => seek(0));
get('speed').addEventListener('change', (event) => { speed = Number((event.target as HTMLSelectElement).value); previous = performance.now(); });
progress.addEventListener('input', () => seek(Number(progress.value) / 100 * total));
get('steps').addEventListener('click', (event) => {
  const button = (event.target as Element).closest<HTMLButtonElement>('[data-step]');
  if (button) { const index = Number(button.dataset.step); seek(index * duration); get('steps').querySelector<HTMLButtonElement>(`[data-step="${index}"]`)?.focus({ preventScroll: true }); }
});
screen.addEventListener('input', (event) => {
  if ((event.target as HTMLElement).id !== 'file-search') return;
  setPlaying(false); query = (event.target as HTMLInputElement).value;
  get('file-list').innerHTML = rows();
});
screen.addEventListener('click', (event) => {
  const button = (event.target as Element).closest<HTMLButtonElement>('button');
  if (!button || button.disabled) return;
  if (button.dataset.file !== undefined) {
    setPlaying(false); selected = Number(button.dataset.file);
    update();
    screen.querySelector<HTMLButtonElement>(`[data-file="${selected}"]`)?.focus({ preventScroll: true });
    return;
  }
  const command = button.dataset.action;
  if (!command) return;
  if (command === 'restart') seek(0);
  else if (command === 'lock') {
    setPlaying(false); desktop.cancel(); manual = true; elapsed = total; update(); setPlaying(false);
    get('vault-folder').focus({ preventScroll: true }); return;
  }
  else if (command === 'export') { transfer('export', selected); return; }
  else if (command === 'import') { transfer('import', files.findIndex((_, index) => !importedIds.has(index))); return; }
  else if (command === 'next' && manual) { elapsed = duration; update(); setPlaying(false); }
  else seek((current + 1) * duration);
  screen.querySelector<HTMLElement>('.spotlight, .file-browser')?.focus({ preventScroll: true });
});
document.addEventListener('visibilitychange', () => { if (document.hidden) setPlaying(false); });
update(); setPlaying(false);
if (import.meta.hot) import.meta.hot.dispose(() => { cancelAnimationFrame(frame); desktop.cancel(); });
