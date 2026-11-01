#!/usr/bin/env python3
"""Desktop launch E2E tests. Real cross-user run0 cases run in the isolated NixOS VM."""
import os
from pathlib import Path
import secrets
import shlex
import sys
import unittest

try:
    import kitty.remote_control
except ImportError:
    os.execvp('kitty', ['kitty', '+runpy', 'import runpy; runpy.run_path(' + repr(str(Path(__file__).resolve())) + ", run_name='__main__')", *sys.argv[1:]])

sys.path.insert(0, str(Path(__file__).resolve().parent))
from integration import KittySSHCase, ROOT, BINARY


class DesktopTests(KittySSHCase):
    def action(self, mode, *args):
        old = {w['id'] for w in self.windows()}
        self.rpc('kitten', dict(match=f'id:{self.first}', kitten=str(ROOT/'kitty/kittyplex_launch.py'), args=[mode,*args]))
        return self.wait_for(lambda: next((w for w in self.windows() if w['id'] not in old and 'kittyplex_launch_origin' in w.get('user_vars',{})),None))['id']

    def launched_view(self, mode, *args):
        old = {w['id'] for w in self.views()}
        carrier = self.action(mode,*args)
        view = self.wait_for(lambda:next((w for w in self.views() if w['id'] not in old),None),timeout=35)['id']
        self.wait_for(lambda:self.text(view).strip())
        return carrier,view

    def command(self, window, command):
        token=secrets.token_hex(8);marker='DONE_'+token
        self.send(window,command+"; printf '\\nDONE_%s\\n' "+token+'\n')
        self.wait_text(window,marker)
        self.wait_for(lambda:self.text(window).rsplit(marker,1)[-1].strip())
        return self.text(window)

    def quit_launch(self, carrier, view):
        self.manage(view,'quit')
        self.wait_for(lambda:all(w['id'] not in (carrier,view) for w in self.windows()))
        self.wait_for(lambda:self.active_window_id()==self.first)

    def test_probe_is_read_only_and_restores_shell(self):
        before={w['id'] for w in self.windows()}
        text=self.command(self.first,'kittyplex doctor')
        self.assertIn('desktop API 1',text)
        self.assertEqual({w['id'] for w in self.windows()},before)
        self.assertIn('AFTER_PROBE',self.command(self.first,"printf 'AFTER_%s\\n' PROBE"))

    def test_local_action_from_managed_view_is_independent(self):
        carrier,view=self.launched_view('start')
        first=next(w for w in self.views() if w['id']==self.first)
        new=next(w for w in self.views() if w['id']==view)
        self.assertNotEqual(first['user_vars']['kittyplex_group'],new['user_vars']['kittyplex_group'])
        self.assertIn('@',new['user_vars']['kittyplex_context'])
        self.send(view,"printf '\\033]2;desktop-title\\007'; sleep 30\n")
        self.wait_for(lambda:'desktop-title' in next(w for w in self.views() if w['id']==view)['tab_title'])
        self.assertIn('@',next(w for w in self.views() if w['id']==view)['tab_title'])
        self.send(view,'\x03')
        self.quit_launch(carrier,view)

    def ssh_options(self):
        return [*self.fixture.ssh[1:-1]]

    def test_ssh_automatic_target_start_and_cleanup(self):
        carrier,view=self.launched_view('ssh','--remote-binary',str(BINARY),self.fixture.ssh[-1],'--',*self.ssh_options())
        text=self.command(view,"printf 'TARGET_%s_%s\\n' \"$USER\" \"$HOME\"")
        self.assertIn('TARGET_'+os.environ['USER']+'_',text)
        self.quit_launch(carrier,view)

    def test_missing_remote_package_stays_readable_until_dismissed(self):
        carrier=self.action('ssh','--remote-binary','/nonexistent/kittyplex',self.fixture.ssh[-1],'--',*self.ssh_options())
        self.wait_text(carrier,'launch failed',timeout=25)
        self.assertEqual(len(self.views()),1)
        self.send(carrier,'\n')
        self.wait_for(lambda:all(w['id']!=carrier for w in self.windows()))

    def test_cancel_transport_keeps_diagnostics_until_dismissed(self):
        carrier = self.action('ssh', 'unused', '--', '-v', '-oProxyCommand=sleep 30')
        self.wait_text(carrier, 'Executing proxy command')
        self.send(carrier, '\x03')
        self.wait_text(carrier, 'launch failed')
        self.assertEqual(len(self.views()), 1)
        self.send(carrier, '\n')
        self.wait_for(lambda: all(w['id'] != carrier for w in self.windows()))
        self.wait_for(lambda: self.active_window_id() == self.first)

    @unittest.skipUnless(os.environ.get('KITTYPLEX_TEST_RUN0_USER'),'real run0 runs only in the isolated NixOS VM')
    def test_real_run0_identity_shell_home_and_pty_modes(self):
        # Accounts and authorization are provisioned by nix/desktop-test.nix.
        for user,mode,prompt,home,shell in [
            ('root','--pty-late','ROOT_READY>','/root','bash'),
            (os.environ['KITTYPLEX_TEST_RUN0_USER'],'--pty','BOB_READY>','/home/bob','zsh'),
            ('carol','--pty-late','CAROL_READY>','/home/carol','fish'),
        ]:
            args=['--no-ask-password',mode]
            if user!='root':args+=['-u',user]
            carrier,view=self.launched_view('run0',*args)
            self.wait_text(view,prompt)
            text=self.command(view,"printf 'IDENT_%s_%s_%s\\n' \"$USER\" \"$HOME\" \"$SHELL\"")
            self.assertIn('IDENT_'+user+'_'+home+'_',text)
            self.assertIn(shell,text)
            actual=self.command(view,'printf "OWNER_%s_%s\\n" "$(id -un)" "$(stat -c %U "$KITTYPLEX_SOCKET")"')
            self.assertIn("OWNER_"+user+"_"+user,actual)
            context=next(w for w in self.views() if w['id']==view)['user_vars']['kittyplex_context']
            self.assertTrue(context.startswith(user+'@'),context)
            for w in self.views():
                status=Path('/proc/'+str(w['pid'])+'/status').read_text()
                self.assertEqual(int(next(line for line in status.splitlines() if line.startswith('Uid:')).split()[1]),os.getuid())
            self.quit_launch(carrier,view)

    @unittest.skipUnless(os.environ.get('KITTYPLEX_TEST_RUN0_USER'),'real run0 runs only in the isolated NixOS VM')
    def test_run0_explicit_group_directory_and_job_control(self):
        carrier,view=self.launched_view('run0','-u',os.environ['KITTYPLEX_TEST_RUN0_USER'],'-g','users','-D','/tmp','--pty','--no-ask-password')
        self.wait_text(view,'BOB_READY>')
        text=self.command(view,'printf "LOCATION_%s_%s\\n" "$PWD" "$(id -gn)"')
        self.assertIn('LOCATION_/tmp_users',text)
        self.send(view,"printf 'JOB_STARTED_%s\\n' READY; sleep 30\n")
        self.wait_text(view,'JOB_STARTED_READY')
        self.send(view,'\x1a')
        self.wait_text(view,'suspended')
        self.assertIn('JOB_CONTROL_RESTORED',self.command(view,'kill %1; printf "JOB_CONTROL_%s\\n" RESTORED'))
        self.quit_launch(carrier,view)


if __name__=='__main__':
    result=unittest.main(verbosity=2,exit=False).result
    if not result.wasSuccessful():raise RuntimeError('desktop tests failed')
