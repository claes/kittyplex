#!/usr/bin/env python3
"""Compare real Kitty behavior over ordinary SSH with termlab's session views."""
import base64
import json
import os
from pathlib import Path
import secrets
import shlex
import shutil
import sys
import unittest

try:
    import kitty.remote_control
except ImportError:
    os.execvp('kitty', ['kitty', '+runpy', 'import runpy; runpy.run_path(' + repr(str(Path(__file__).resolve())) + ", run_name='__main__')", *sys.argv[1:]])

sys.path.insert(0, str(Path(__file__).resolve().parent))
from integration import KittySSHCase


class CompatibilityTests(KittySSHCase):
    features = True
    def setUp(self):
        super().setUp()
        self.media = self.fixture.directory / 'feature-assets'
        self.run_command(self.first, 'stty -echo')

    def run_command(self, window, command, timeout=20):
        token = secrets.token_hex(8)
        status = self.fixture.directory / (token + '.status')
        error = self.fixture.directory / (token + '.stderr')
        # stdout stays a terminal, as it does for the user's exact icat command.
        prompt='tl-done-'+token+'> '
        self.send(window, f"PS1={shlex.quote(prompt)}; {command} 2>{shlex.quote(str(error))}; printf '%s' $? >{shlex.quote(str(status))}\n")
        self.wait_for(lambda: status.exists() and status.read_text().strip(), timeout)
        self.assertEqual(status.read_text(), '0', error.read_text())
        self.wait_for(lambda:self.text(window).rstrip().endswith(prompt.rstrip()))

    def python(self, window, code, timeout=20):
        self.run_command(window, shlex.join(['python3', '-c', code]), timeout)

    def remote_geometry(self, window):
        result = self.fixture.directory / (secrets.token_hex(8) + '.geometry')
        self.python(window, "import array,fcntl,termios,json; from pathlib import Path; "
                    "s=array.array('H',[0,0,0,0]); fcntl.ioctl(1,termios.TIOCGWINSZ,s); "
                    f"Path({str(result)!r}).write_text(json.dumps(list(s)))")
        return json.loads(result.read_text())

    def direct(self):
        window = int(self.rpc('launch', dict(type='tab', args=self.fixture.ssh[:-1] + ['-tt', self.fixture.ssh[-1],
                     'exec ' + shlex.join(['env', 'PS1=DIRECT_PROMPT> ', 'PATH='+os.environ['PATH'], shutil.which('bash'), '--noprofile', '--norc', '-i'])])))
        self.wait_text(window, 'DIRECT_PROMPT>')
        self.run_command(window, 'stty -echo')
        return window

    def query(self, window, request, terminator, setup=b''):
        result = self.fixture.directory / (secrets.token_hex(8) + '.reply')
        endings=(terminator,) if isinstance(terminator,bytes) else tuple(terminator)
        self.python(window, f'''import os,termios,tty,select,time
from pathlib import Path
f=os.open('/dev/tty',os.O_RDWR); saved=termios.tcgetattr(f); data=b''
try:
 tty.setraw(f); os.write(f,{setup + request!r}); end=time.monotonic()+5
 while time.monotonic()<end:
  if select.select([f],[],[],.1)[0]:
   data+=os.read(f,65536)
   if any(t in data for t in {endings!r}): break
finally:
 termios.tcsetattr(f,termios.TCSANOW,saved); os.close(f)
Path({str(result)!r}).write_text(data.hex())
''')
        return bytes.fromhex(result.read_text())

    def test_feature_demo_command(self):
        before = self.probe(self.first, "graphics")["count"]
        self.run_command(self.first, "termlab-features")
        self.wait_for(lambda: self.probe(self.first, "graphics")["count"] > before)

    def test_full_geometry_and_icat_default(self):
        for window in (self.direct(), self.first):
            expected = self.probe(window, 'geometry')
            self.assertTrue(all(expected), expected)
            self.assertEqual(self.remote_geometry(window), expected)
            before = self.probe(window, 'graphics')['count']
            self.run_command(window, shlex.join(['kitten', 'icat', str(self.media / 'test.png')]))
            self.wait_for(lambda: self.probe(window, 'graphics')['count'] > before)

    def test_resize_and_pixel_only_change(self):
        other = self.create(self.first)
        for window in (self.first, other):
            self.run_command(window, 'stty -echo')
        before = self.probe(self.first, 'geometry')
        # Pixel dimensions change even though the grid remains fixed. Use the
        # actual view's local PTY as the oracle, not OS-window/tab-bar dimensions.
        self.probe(self.first, 'pixel-resize', before[2] + 7, before[3] + 9)
        self.wait_for(lambda: self.remote_geometry(self.first) == self.probe(self.first, 'geometry'))
        self.assertEqual(self.probe(self.first, 'geometry')[:2], before[:2])
        self.assertNotEqual(self.probe(self.first, 'geometry')[2:], before[2:])
        self.rpc('resize-os-window',dict(match=f'id:{other}',action='resize',unit='cells',width=100,height=35))
        for window in (self.first, other):
            self.wait_for(lambda: self.remote_geometry(window) == self.probe(window, 'geometry'))
        self.rpc('set-font-size',dict(size=16))
        for window in (self.first, other):
            self.wait_for(lambda: self.remote_geometry(window) == self.probe(window, 'geometry'))

    def test_graphics_streaming_isolation_and_clear(self):
        second = self.create(self.first)
        self.run_command(second, 'stty -echo')
        for window, width, color in ((self.first, 2, b'\xff\x00\x00'), (second, 3, b'\x00\x00\xff')):
            command = f'\x1b_Ga=T,f=24,s={width},v=2,i=42;'.encode() + base64.b64encode(color * width * 2) + b'\x1b\\'
            reply = self.query(window, command, b'\x1b\\')
            self.assertIn(b'i=42;OK', reply)
            self.wait_for(lambda: self.probe(window,'graphics',42).get('image',{}).get('width') == width)
        self.manage(second,'previous')
        self.assertEqual(self.probe(self.first,'graphics',42)['image']['width'],2)
        self.assertEqual(self.probe(second,'graphics',42)['image']['width'],3)
        self.run_command(self.first,'kitten icat --clear')
        cleared = self.probe(self.first, 'graphics', 42).get('image', {})
        self.assertEqual(cleared.get('refs.count', 0), 0)
        self.assertEqual(self.probe(second,'graphics',42)['image']['width'],3)
        self.run_command(self.first,shlex.join(['kitten','icat','--image-id=99','--transfer-mode=stream',str(self.media/'large.png')]), timeout=35)
        self.wait_for(lambda:self.probe(self.first,'graphics',99).get('image',{}).get('width',0)>0,timeout=20)
        self.run_command(self.first,shlex.join(['kitten','icat','--unicode-placeholder','--image-id=100','--transfer-mode=stream',str(self.media/'test.png')]))
        self.wait_for(lambda:self.probe(self.first,'graphics',100).get('image') is not None)
        self.assertIn('\U0010eeee',self.text(self.first))
        self.run_command(self.first,shlex.join(['kitten','icat','--image-id=101','--loop=1',str(self.media/'animation.gif')]), timeout=25)
        self.wait_for(lambda: self.probe(self.first, 'graphics', 101).get('image', {}).get('extra_frame_count', 0) > 0)

    def test_graphics_replies_and_alternate_screen(self):
        second=self.create(self.first)
        # A missing local file emulates the negative probe needed for stream
        # fallback. It does not pretend localhost has a separate filesystem.
        missing='/nonexistent/termlab-graphics-'+secrets.token_hex(8)
        command=b'\x1b_Ga=q,t=f,f=100,i=123;' + base64.b64encode(missing.encode()) + b'\x1b\\'
        reply=self.query(self.first,command,b'\x1b\\')
        self.assertIn(b'i=123;',reply)
        self.assertNotIn(b';OK',reply)
        self.assertNotIn('ENOENT',self.text(second))
        self.run_command(self.first,"printf '\\033[?1049h'")
        self.run_command(self.first,shlex.join(['kitten','icat','--image-id=77','--transfer-mode=stream',str(self.media/'test.png')]))
        self.wait_for(lambda:self.probe(self.first,'graphics',77).get('image') is not None)
        self.run_command(self.first,"printf '\\033[?1049l'")
        self.wait_for(lambda:not next(w for w in self.views() if w['id']==self.first)['in_alternate_screen'])

    def test_terminal_queries_colors_and_metadata(self):
        for window in (self.direct(),self.first):
            for request, terminal, prefix in [(b'\x1b[6n',b'R',b'\x1b['),(b'\x1b[14t',b't',b'\x1b[4;'),
                                               (b'\x1b[16t',b't',b'\x1b[6;'),(b'\x1b[c',b'c',b'\x1b[?'),
                                               (b'\x1b]10;?\x1b\\',b'\x1b\\',b'\x1b]10;'),
                                               (b'\x1b]99;i=audit:p=?;\x1b\\',b'\x1b\\',b'i=audit:p=?')]:
                self.assertIn(prefix,self.query(window,request,terminal))
            self.run_command(window,"printf '\\033]2;compatibility-title\\007\\033]7;file://example.invalid/remote/path\\007\\033]133;A\\007\\033]1337;SetUserVar=audit=b2s=\\007'")
            self.wait_for(lambda:next(w for w in self.windows() if w['id']==window)['title']=='compatibility-title')
            self.assertEqual(next(w for w in self.windows() if w['id']==window)['user_vars']['audit'],'ok')
            self.assertIn('/remote/path',self.probe(window,'metadata')['cwd'])
            self.assertTrue(self.probe(window,'metadata')['at_prompt'])
            self.run_command(window,"printf '\\033[38;2;17;34;51mTRUE_COLOR\\033[0m\\033]8;;https://example.invalid/audit\\033\\\\LINK\\033]8;;\\033\\\\'")
            ansi=self.rpc('get-text',dict(match=f'id:{window}',extent='all',ansi=True))
            self.assertIn('TRUE_COLOR',ansi)
            self.assertRegex(ansi, r'38[:;]2[:;]17[:;]34[:;]51m')
            self.assertIn('\x1b]8;;https://example.invalid/audit\x1b\\', ansi)
            self.assertIn('LINK',ansi)

    def capture_input(self,window,setup,send):
        root=self.fixture.directory/secrets.token_hex(8)
        ready=Path(str(root)+'.ready');result=Path(str(root)+'.input')
        marker='CAPTURE_READY_'+secrets.token_hex(8)
        code=f'''import os,termios,tty,select,time
from pathlib import Path
f=os.open('/dev/tty',os.O_RDWR); old=termios.tcgetattr(f); data=b''
try:
 tty.setraw(f); os.write(f,{setup!r}+{marker.encode()!r}); Path({str(ready)!r}).touch(); end=time.monotonic()+3
 while time.monotonic()<end:
  if select.select([f],[],[],.1)[0]: data+=os.read(f,8192)
finally:
 os.write(f,b'\\x1b[<u\\x1b[?1l\\x1b[?2004l\\x1b[?1004l\\x1b[?1000l\\x1b[?1006l'); termios.tcsetattr(f,termios.TCSANOW,old); os.close(f)
Path({str(result)!r}).write_text(data.hex())
'''
        prompt='tl-captured-'+secrets.token_hex(6)+'> '
        self.send(window,'PS1='+shlex.quote(prompt)+'; '+shlex.join(['python3','-c',code])+'\n')
        self.wait_for(ready.exists)
        self.wait_for(lambda:self.text(window).rstrip().endswith(marker))
        send()
        self.wait_for(result.exists)
        self.wait_for(lambda:self.text(window).rstrip().endswith(prompt.rstrip()))
        return bytes.fromhex(result.read_text())

    def test_keyboard_paste_focus_and_mouse_modes(self):
        baseline=self.direct()
        for window in (baseline,self.first):
            reply=self.capture_input(window,b'\x1b[>9u',lambda:self.rpc('send-key',dict(match=f'id:{window}',keys=['ctrl+a'])))
            self.assertIn(b'\x1b[97;5u',reply)
            reply=self.capture_input(window,b'\x1b[?1h',lambda:self.rpc('send-key',dict(match=f'id:{window}',keys=['up'])))
            self.assertIn(b'\x1bOA',reply)
            reply=self.capture_input(window,b'\x1b[?2004h',lambda:self.rpc('send-text',dict(match=f'id:{window}',data='text:PASTE',bracketed_paste='auto')))
            self.assertIn(b'\x1b[200~PASTE\x1b[201~',reply)
        second=self.create(self.first)
        self.manage(second,'previous')
        reply=self.capture_input(self.first,b'\x1b[?1004h',lambda:(self.rpc('focus-window',dict(match=f'id:{second}')),self.rpc('focus-window',dict(match=f'id:{self.first}'))))
        self.assertIn(b'\x1b[O',reply)
        self.assertIn(b'\x1b[I',reply)
        self.run_command(self.first,"printf '\\033[?1000h\\033[?1006h'")
        for mode in (1000, 1006):
            reply = self.query(self.first, f'\x1b[?{mode}$p'.encode(), b'$y')
            self.assertIn(f'\x1b[?{mode};1$y'.encode(), reply)
        self.run_command(self.first,"printf '\\033[?1000l\\033[?1006l'")

    def test_clipboard_write_obeys_native_kitty_policy(self):
        # Xvfb owns a private selection; never inspect a real desktop clipboard.
        if not self.xvfb:
            self.skipTest('clipboard audit requires the isolated Xvfb display')
        for window in (self.direct(),self.first):
            token='termlab-clipboard-'+secrets.token_hex(8)
            sequence=b'\x1b]52;c;'+base64.b64encode(token.encode())+b'\x07'
            self.python(window,f'import os; os.write(1,{sequence!r})')
            self.wait_for(lambda:self.probe(window,'clipboard')==token)


    def test_clipboard_read_requires_explicit_native_permission(self):
        if not self.xvfb:
            self.skipTest('clipboard audit requires the isolated Xvfb display')
        baseline=self.direct()
        token='private-audit-'+secrets.token_hex(8)
        sequence=b'\x1b]52;c;'+base64.b64encode(token.encode())+b'\x07'
        self.python(baseline,f'import os; os.write(1,{sequence!r})')
        config=self.fixture.directory/'kitty/kitty.conf'
        for permission in ('write-clipboard write-primary read-clipboard','write-clipboard write-primary'):
            with config.open('a') as stream:stream.write('\nclipboard_control '+permission+'\n')
            self.rpc('load-config',dict(paths=[str(config)],override=[],ignore_overrides=False))
            replies=[self.query(window,b'\x1b]52;c;?\x07',(b'\x07',b'\x1b\\')) for window in (baseline,self.first)]
            if 'read-clipboard' in permission:
                for reply in replies:self.assertIn(base64.b64encode(token.encode()),reply)
            else:
                for reply in replies:self.assertNotIn(base64.b64encode(token.encode()),reply)



if __name__=='__main__':
    result=unittest.main(verbosity=2,exit=False).result
    if not result.wasSuccessful():
        raise RuntimeError('Kitty compatibility tests failed')
