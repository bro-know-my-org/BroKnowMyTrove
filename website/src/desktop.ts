export type Direction = 'import' | 'export';
type DesktopFile = { name: string; kind: string; color: string };
export function mountDesktop(files: readonly DesktopFile[], icon: (name: string) => string, transfer: (kind: Direction, index: number) => void, allowed: (kind: Direction, index: number) => boolean) {
  const root = document.getElementById('desktop')!;
  const ghost = document.getElementById('drag-ghost')!;
  const trail = document.querySelector<SVGPathElement>('#drag-trail path')!;
  const output = document.getElementById('desktop-output')!;
  const hint = document.getElementById('desktop-hint')!;
  const folder = document.querySelector<HTMLElement>('.vault-folder-icon')!;
  let storedCount = 0, exportedCount = 0;
  function activity(kind: Direction) {
    if (matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    folder.getAnimations().forEach(animation => animation.cancel());
    folder.animate([
      { transform: 'translateY(0) scale(1)' },
      { transform: `translateY(${kind === 'import' ? '-5' : '-2'}px) scale(1.06)`, offset: .35 },
      { transform: 'translateY(0) scale(1)' },
    ], { duration: 750, easing: 'ease-out' });
    folder.querySelector('.vault-folder-front')!.animate([
      { boxShadow: '0 0 0 0 #a5dbff00' },
      { boxShadow: `0 0 24px 7px ${kind === 'import' ? '#a5dbff88' : '#c4e7bd88'}`, offset: .35 },
      { boxShadow: '0 0 0 0 #a5dbff00' },
    ], { duration: 950, easing: 'ease-out' });
  }
  type Pick = { kind: Direction; index: number; element: HTMLElement };
  let picked: Pick | null = null;
  let drag: (Pick & { x: number; y: number; pointer: number; moved: boolean }) | null = null;
  let ignoreClickUntil = 0;
  const documentIcon = (index: number) => `<span class="desk-document ${files[index].color}">${icon('file')}<small>${files[index].kind}</small></span>`;
  const sources = document.getElementById('desktop-files')!;
  sources.innerHTML = files.map((file, index) => `<button type="button" class="desktop-file" data-source="${index}" aria-label="桌面文件 ${file.name}，拖入文件库"><span class="desk-icon">${documentIcon(index)}<i aria-hidden="true" hidden>✓</i></span><span>${file.name}</span></button>`).join('');
  const sourceButtons = [...sources.querySelectorAll<HTMLButtonElement>('[data-source]')];
  const destination = (kind: Direction) => kind === 'import' ? root.querySelector<HTMLElement>('.file-browser') : output;
  function clear() {
    ghost.hidden = true; trail.setAttribute('d', '');
    root.querySelectorAll('.drop-hover').forEach(element => element.classList.remove('drop-hover'));
  }
  function cancel() {
    if (drag && root.hasPointerCapture(drag.pointer)) root.releasePointerCapture(drag.pointer);
    drag = null; picked = null; clear();
  }
  function show(kind: Direction, index: number, start: { x: number; y: number }, end: { x: number; y: number }, amount: number, count = 1) {
    const box = root.getBoundingClientRect();
    const x = start.x + (end.x - start.x) * amount;
    const y = start.y + (end.y - start.y) * amount - Math.sin(amount * Math.PI) * 35;
    ghost.hidden = false;
    if (ghost.dataset.file !== `${kind}:${index}:${count}`) {
      ghost.innerHTML = `<span class="ghost-cursor">➤</span>${documentIcon(index)}<span>${count > 1 ? `${count} 个文件` : files[index].name}</span><b>${kind === 'import' ? '+ 导入' : '+ 导出'}</b>`;
      ghost.dataset.file = `${kind}:${index}:${count}`;
    }
    ghost.style.transform = `translate(${x - box.left}px, ${y - box.top}px)`;
    trail.setAttribute('d', `M${start.x - box.left},${start.y - box.top} Q${(start.x + end.x) / 2 - box.left},${Math.min(start.y, end.y) - box.top - 50} ${end.x - box.left},${end.y - box.top}`);
    destination(kind)?.classList.add('drop-hover');
  }
  function identify(target: Element): Pick | null {
    const element = target.closest<HTMLElement>('[data-source], [data-file]');
    if (!element) return null;
    return { kind: element.dataset.source !== undefined ? 'import' : 'export', index: Number(element.dataset.source ?? element.dataset.file), element };
  }
  root.addEventListener('pointerdown', event => {
    if (event.button !== 0) return;
    const file = identify(event.target as Element);
    if (file && allowed(file.kind, file.index)) drag = { ...file, x: event.clientX, y: event.clientY, pointer: event.pointerId, moved: false };
    else if (file) transfer(file.kind, file.index);
  });
  root.addEventListener('pointermove', event => {
    if (!drag || event.pointerId !== drag.pointer) return;
    if ((event.buttons & 1) === 0) { cancel(); return; }
    if (!drag.moved && Math.hypot(event.clientX - drag.x, event.clientY - drag.y) < 6) return;
    drag.moved = true; event.preventDefault(); root.setPointerCapture(event.pointerId);
    root.dispatchEvent(new Event('manual-drag'));
    show(drag.kind, drag.index, { x: drag.x, y: drag.y }, { x: event.clientX, y: event.clientY }, 1);
    const target = destination(drag.kind)?.getBoundingClientRect();
    const valid = target && event.clientX >= target.left && event.clientX <= target.right && event.clientY >= target.top && event.clientY <= target.bottom && allowed(drag.kind, drag.index);
    destination(drag.kind)?.classList.toggle('drop-hover', Boolean(valid));
    ghost.classList.toggle('invalid-drop', !valid);
  });
  root.addEventListener('pointerup', event => {
    if (!drag || event.pointerId !== drag.pointer) return;
    const saved = drag, target = destination(saved.kind)?.getBoundingClientRect();
    const hit = target && event.clientX >= target.left && event.clientX <= target.right && event.clientY >= target.top && event.clientY <= target.bottom;
    cancel();
    if (!saved.moved) return;
    ignoreClickUntil = performance.now() + 250;
    if (hit) transfer(saved.kind, saved.index);
    else hint.textContent = '已取消拖动，文件仍在原处。';
  });
  root.addEventListener('pointercancel', cancel);
  root.addEventListener('lostpointercapture', event => {
    if (event.target === root && drag?.pointer === event.pointerId) cancel();
  });
  root.addEventListener('keydown', event => { if (event.key === 'Escape') { cancel(); hint.textContent = '已取消选择，文件仍在原处。'; } });
  root.addEventListener('click', event => {
    if (performance.now() < ignoreClickUntil) { event.preventDefault(); event.stopPropagation(); return; }
    const target = event.target as Element;
    if (picked && destination(picked.kind)?.contains(target)) {
      const saved = picked; picked = null; transfer(saved.kind, saved.index); event.stopPropagation(); return;
    }
    picked = identify(target);
    if (picked) hint.textContent = `已选择「${files[picked.index].name}」，拖动或点击${picked.kind === 'import' ? '文件库中央空白处以导入' : '桌面导出区域以导出'}。`;
  }, true);
  root.addEventListener('keydown', event => {
    if ((event.key === 'Enter' || event.key === ' ') && picked && event.target === destination(picked.kind)) {
      event.preventDefault(); const saved = picked; picked = null; transfer(saved.kind, saved.index);
    }
  });
  root.addEventListener('dragstart', event => event.preventDefault());
  return {
    cancel,
    activity,
    note(text: string) { hint.textContent = text; },
    sync(imported: ReadonlySet<number>, exported: ReadonlySet<number>) {
      if (imported.size > storedCount) activity('import');
      else if (exported.size > exportedCount) activity('export');
      storedCount = imported.size; exportedCount = exported.size;
      sourceButtons.forEach((button, index) => { button.querySelector<HTMLElement>('i')!.hidden = !imported.has(index); });
      document.getElementById('exported-files')!.innerHTML = [...exported].map(index => `<span class="exported-file">${documentIcon(index)}<span>${files[index].name}</span><small>已导出到桌面</small></span>`).join('');
    },
    animate(kind: Direction | null, amount = 0, index = 0) {
      if (drag?.moved) return;
      clear();
      if (!kind || matchMedia('(prefers-reduced-motion: reduce)').matches) return;
      const source = root.querySelector<HTMLElement>(kind === 'import' ? '[data-source="0"]' : `[data-file="${index}"]`);
      const target = destination(kind);
      if (!source || !target) return;
      const from = source.getBoundingClientRect(), to = target.getBoundingClientRect();
      ghost.classList.remove('invalid-drop');
      show(kind, index, { x: from.left + from.width / 2, y: from.top + from.height / 2 }, { x: to.left + to.width / 2, y: to.top + to.height / 2 }, amount * amount * (3 - 2 * amount), kind === 'import' ? 3 : 1);
    },
  };
}
