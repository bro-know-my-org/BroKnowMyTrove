#!/usr/bin/env python3
"""Linux-only I/O fault test on a newly created 128 MiB loop image; requires root.

Never accepts an existing device. All files/mappings/mounts belong to this invocation.
This simulates block I/O errors, not physical unplugging or real power loss.
"""
import json, os, pathlib, runpy, signal, subprocess, sys, tempfile, time, shutil
if len(sys.argv) != 2 or sys.platform != 'linux':
    raise SystemExit('usage (Linux/root): test-device.py /absolute/path/to/bkm')
assert os.geteuid() == 0
sys.argv[1] = os.path.abspath(sys.argv[1])
call = runpy.run_path(str(pathlib.Path(__file__).with_name('test-cli.py')))['run_cli']
def command(*args, check=True):
    return subprocess.run(args, check=check, capture_output=True, text=True, timeout=30)
task = tempfile.mkdtemp(prefix='bkm-device-', dir='/var/tmp')
root = pathlib.Path(task)
loop = None
mapped = mounted = error_table = False
name = 'bkm-validation-' + str(os.getpid())
mount = root / 'mount'
def reload(table):
    command('dmsetup', 'suspend', '--noflush', '--nolockfs', name)
    try:
        command('dmsetup', 'reload', name, '--table', table)
    finally:
        command('dmsetup', 'resume', name)
try:
    image = root / 'disposable.img'
    with image.open('wb') as file:
        file.truncate(128 * 1024 * 1024)
    device = '/dev/mapper/' + name
    sectors = 128 * 1024 * 1024 // 512
    mount = root / 'mount'; mount.mkdir()
    loop = command('losetup', '--find', '--show', str(image)).stdout.strip()
    linear = f'0 {sectors} linear {loop} 0'
    command('dmsetup', 'create', name, '--table', linear); mapped = True
    command('mkfs.ext4', '-q', '-F', device)
    command('mount', '-o', 'nodelalloc', device, str(mount)); mounted = True
    vault = mount / 'vault'
    call(['create', vault], confirm=True)
    old = json.loads(call(['inspect', vault, '--json'])[0])['head']
    source = root / 'source'
    with source.open('wb') as file:
        file.truncate(64 * 1024 * 1024)
    def fail_device(process):
        global error_table
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if process.poll() is not None: raise AssertionError('import finished before fault')
            os.kill(process.pid, signal.SIGSTOP)
            while True:
                pid, status = os.waitpid(process.pid, os.WUNTRACED | os.WNOHANG)
                if pid:
                    assert os.WIFSTOPPED(status)
                    break
                if time.monotonic() >= deadline: raise AssertionError('stop timeout')
                time.sleep(.001)
            try:
                if any(1048576 < p.stat().st_size < 32 * 1024 * 1024 for p in (vault/'objects').glob('.tmp-*')):
                    reload(f'0 {sectors} error'); error_table = True
                    return
            finally:
                os.kill(process.pid, signal.SIGCONT)
            time.sleep(.002)
        raise AssertionError('no active stream observed')
    _, diagnostic, _ = call(['import', vault, source, 'must not commit'], expected=1, during=fail_device)
    assert error_table
    print('ACTIVE_DEVICE_ERROR_REPORTED', diagnostic.strip(), flush=True)
    reload(linear); error_table = False
    command('umount', str(mount)); mounted = False
    repaired = command('e2fsck', '-fy', device, check=False)
    assert repaired.returncode in (0, 1), repaired.stderr
    command('mount', device, str(mount)); mounted = True
    assert call(['list', vault])[0] == ''
    assert json.loads(call(['inspect', vault, '--json'])[0])['head'] == old
    call(['gc', vault, '--confirm'])
    assert not list((vault/'objects').glob('.tmp-*'))
    identity = call(['import', vault, source, 'retry'])[0].strip()
    exported = root/'exported'
    call(['export', vault, identity, exported])
    assert exported.stat().st_size == source.stat().st_size
    assert exported.read_bytes() == source.read_bytes()
    print('DEVICE_ERROR_RECOVERY_OK: old head retained; fsck/remount/GC/retry/export passed', flush=True)
finally:
    cleanup_errors = []
    def attempt(function, *args):
        try:
            function(*args)
        except Exception as error:
            cleanup_errors.append(str(error))
    if mapped:
        attempt(reload, linear)
        # Even if table replacement failed, resume so unmount cannot wait on suspension.
        attempt(command, 'dmsetup', 'resume', name)
    if mounted:
        attempt(command, 'umount', str(mount))
    if mapped:
        attempt(command, 'dmsetup', 'remove', name)
    if loop:
        attempt(command, 'losetup', '-d', loop)
    # Never recursively delete a path until all kernel resources are confirmed gone.
    try:
        mounted_now = mount.exists() and mount.stat().st_dev != root.stat().st_dev
        mappings = command('dmsetup', 'info', '--columns', '--noheadings', '-o', 'name').stdout.split()
        loops = command('losetup', '--list', '--noheadings', '--output', 'NAME').stdout.split()
        mapped_now = name in mappings
        loop_now = loop in loops
        if mounted_now or mapped_now or loop_now:
            cleanup_errors.append('kernel resources remain attached')
        else:
            shutil.rmtree(root)
    except Exception as error:
        cleanup_errors.append(str(error))
    if cleanup_errors:
        raise RuntimeError(f'Cleanup incomplete; inspect {root}: ' + '; '.join(cleanup_errors))
