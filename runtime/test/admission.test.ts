import assert from 'node:assert/strict';
import { test } from 'node:test';
import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider, fauxAssistantMessage, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import { Runtime } from '../src/runtime.ts';
import { validateAdmission, type Admission } from '../src/admission.ts';
import { RunCapacity, resourceFailure, configuredCapacity } from '../src/run-capacity.ts';
import { acquireCheckout, rootIdentity } from '../src/safety.ts';
import { setCommandPolicy, commandPolicy } from '../src/command/policy.ts';
import { codex, claude } from './support/record-runtime.ts';
import { exec, setup } from './support/safety.ts';

const pause = () => new Promise(resolve => setTimeout(resolve, 20));
async function until(done: () => boolean | Promise<boolean>) { const end=Date.now()+20000; while(!await done()) { assert.ok(Date.now()<end,'Timed out'); await pause(); } }
async function commit(root: string) {
  await writeFile(join(root,'source.txt'),'committed\n');
  await exec('/usr/bin/git',['-C',root,'add','.']);
  await exec('/usr/bin/git',['-C',root,'-c','user.name=Test','-c','user.email=test@invalid','-c','commit.gpgsign=false','commit','-qm','base']);
}
async function admitted(runtime: Runtime, id: string) {
  let result: Admission | undefined;
  await until(async()=>{ const reply:any=await runtime.reply({op:'admission_status',request_id:id}); assert.equal(reply.ok,true,JSON.stringify(reply)); result=reply.result.admission; return result!.state!=='preparing'; });
  assert.equal(result!.state,'ready',JSON.stringify(result)); return result!;
}
class FakeRuntime extends Runtime {
  override async resources() { return {skills:[],templates:[],problems:[],mcpServers:[],mcpEntries:new Map()}; }
}
async function pi(t: Parameters<typeof setup>[0], responses: Parameters<ReturnType<typeof fauxProvider>['setResponses']>[0]) {
  const {root,base}=await setup(t); const store=join(base,'store'); await mkdir(store,{mode:0o700});
  const models=createModels(); const faux=fauxProvider({tokensPerSecond:1e8}); faux.setResponses(responses); models.setProvider(faux.provider);
  const runtime=new FakeRuntime(store,models,new Set(['faux'])); t.after(()=>runtime.close()); return {root,base,store,runtime};
}
const start=(root:string,id:string,harness='pi')=>({op:'create_admitted_thread',request_id:id,checkout:root,placement:'project',harness,provider:harness==='pi'?'faux':harness,model_id:harness==='pi'?'faux-1':harness==='codex'?'gpt-fake':'default',approval:'auto',...(harness==='pi'?{}:{mode:harness==='codex'?'ask':'auto'})});

test('background polls and repeated starts return while placement is still preparing', async t => {
  const { runtime, root } = await pi(t, []);
  const admissions = runtime.admissions;
  const original = admissions['createHere'].bind(admissions);
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  admissions['createHere'] = async (input, identity) => {
    await gate;
    return original(input, identity);
  };
  const request = start(root, 'pending');
  const timeout = setTimeout(release, 3000);
  try {
    await admissions.handle(request, true);
    for (const poll of [{ op: 'admission_status', request_id: request.request_id }, request]) {
      const reply = await admissions.handle(poll, true) as { admission: Admission };
      assert.equal(reply.admission.state, 'preparing');
    }
  } finally {
    clearTimeout(timeout);
    release();
  }
  await admitted(runtime, request.request_id);
});

test('strict admission fields refuse queue, forged identity and protocol fields',()=>{
  for(const request of [{...start('/repo','one'),placement:'queue'},{...start('/repo','one'),protocol_version:6},{...start('/repo','one'),extra:true},{op:'set_project_isolation',checkout:'/repo',identity:{dev:'01',ino:'2'},auto_worktree:true},{...start('/repo','one'),source_thread_id:'existing'},{op:'admission_info',checkout:'/repo/../other'}]) assert.throws(()=>validateAdmission(request));
});
test('capacity has no eight-checkout default and explicit configured refusals',()=>{
  assert.equal(configuredCapacity(''),undefined); new RunCapacity().admit(1000);
  assert.throws(()=>new RunCapacity(20).admit(20),/Nothing was queued/);
  for(const value of ['0','-1','1.5','Infinity','9007199254740992']) assert.throws(()=>configuredCapacity(value));
  for(const code of ['EMFILE','ENFILE','EAGAIN','ENOMEM','ENOSPC','EDQUOT']) assert.match(resourceFailure({code})!.message,/No automatic retry/);
});
test('twenty independent checkouts run, including nine and twenty admissions; one root stays one writer',async t=>{
  let release!:()=>void; const held=new Promise<void>(resolve=>release=resolve); let entered=0;
  const h=await pi(t,Array.from({length:20},()=>async()=>{entered++;await held;return fauxAssistantMessage('done');}));
  const threads:string[]=[];
  try {
    for(let i=0;i<20;i++) {
      const root=i===0?h.root:join(h.base,`repo-${i}`); if(i) {await mkdir(root);await exec('/usr/bin/git',['init','-q',root]);}
      const created:any=await h.runtime.reply({protocol_version:6,op:'create_thread',checkout:root,request_id:`c${i}`,harness:'pi',provider:'faux',model_id:'faux-1'});assert.equal(created.ok,true,JSON.stringify(created));
      const id=created.result.thread.id;threads.push(id);
      const reply=await h.runtime.reply({protocol_version:6,op:'submit',thread_id:id,request_id:`run${i}`,text:'hold',approval:'auto'});assert.equal(reply.ok,true,JSON.stringify(reply));
      if(i===8||i===19) assert.equal(h.runtime.active.size,i+1);
    }
    await until(()=>entered===20);
    const other:any=await h.runtime.reply({protocol_version:6,op:'create_thread',checkout:h.root,request_id:'other',harness:'pi',provider:'faux',model_id:'faux-1'});
    const refused=await h.runtime.reply({protocol_version:6,op:'submit',thread_id:other.result.thread.id,request_id:'same',text:'do not run',approval:'auto'});
    assert.equal(refused.error?.code,'busy');assert.equal(h.runtime.store.inputs(other.result.thread.id).length,0);
  } finally {release();await until(()=>h.runtime.active.size===0);}
  assert.equal(threads.filter(id=>h.runtime.snapshot(id).status==='idle').length,20);
});
for(const harness of ['pi','codex','claude'] as const) test(`busy choice, authorized automatic worktree, cold HEAD and exact ${harness} policy`,async t=>{
  const contexts:string[]=[];
  const h=harness==='pi'?await pi(t,[context=>{contexts.push(JSON.stringify(context.messages));return fauxAssistantMessage([fauxToolCall('write',{path:'pi-owned.txt',content:'isolated'},{id:'write'})],{stopReason:'toolUse'});}, fauxAssistantMessage('done'), context=>{contexts.push(JSON.stringify(context.messages));return fauxAssistantMessage('done');}]):await (harness==='codex'?codex(t,'turn',true):claude(t));
  await chmod(h.store,0o700);await commit(h.root);await writeFile(join(h.root,'source.txt'),'dirty');await writeFile(join(h.root,'untracked.txt'),'keep');
  const identity=await rootIdentity(h.root);setCommandPolicy(h.runtime.store,h.root,identity,'sandboxed');
  const lease=await acquireCheckout(h.root);
  let ready:Admission;
  try {
    const request=start(h.root,'choice',harness);
    const choice:any=await h.runtime.reply(request);assert.equal(choice.result.admission.state,'choice');
    assert.equal(h.runtime.store.db.prepare('SELECT count(*) AS n FROM thread_admissions').get()!.n,0);
    const chosen={...request,placement:'worktree'};
    const replies:any[]=await Promise.all([h.runtime.reply(chosen),h.runtime.reply(chosen)]);assert.equal(replies[0].ok,true,JSON.stringify(replies));
    ready=await admitted(h.runtime,'choice');
    assert.equal((await h.runtime.reply({...chosen,approval:'ask'})).error?.code,'invalid_request');
    assert.equal((await h.runtime.reply(chosen) as any).result.admission.thread_id,ready.thread_id);
    const info:any=await h.runtime.reply({op:'admission_info',checkout:h.root});assert.equal(info.result.busy,true);assert.equal(info.result.auto_worktree,false);assert.equal(info.result.queue,true);
    assert.equal((await h.runtime.reply({op:'set_project_isolation',checkout:h.root,identity:{...identity,ino:'0'},auto_worktree:true})).ok,false);
    assert.equal((await h.runtime.reply({op:'set_project_isolation',checkout:h.root,identity,auto_worktree:true})).ok,true);
    await h.runtime.reply(start(h.root,'automatic',harness));const automatic=await admitted(h.runtime,'automatic');assert.ok(automatic.worktree);assert.notEqual(automatic.thread_id,ready.thread_id);
    const waiting={...start(h.root,'explicit-wait',harness),placement:'wait'};
    assert.equal((await h.runtime.reply(waiting)).ok,true);const inPlace=await admitted(h.runtime,'explicit-wait');
    assert.equal(inPlace.checkout,h.root);assert.equal(inPlace.worktree,null);
    const queued=await h.runtime.reply({op:'submit_checkout',thread_id:inPlace.thread_id,request_id:'queued-first',text:'wait here',approval:'auto'});
    assert.equal(queued.ok,true,JSON.stringify(queued));assert.equal(h.runtime.store.waiters.get(inPlace.thread_id!,'queued-first')!.state,'waiting');
    await h.runtime.reply({op:'cancel_checkout_wait',thread_id:inPlace.thread_id,request_id:'queued-first'});
  } finally {await lease.close();}
  const target=ready!.checkout;const id=ready!.thread_id!; const snapshot=h.runtime.snapshot(id);
  assert.equal(snapshot.checkout,target);assert.equal(snapshot.harness?.id??'pi',harness);assert.equal(snapshot.approval_mode,'auto');
  assert.equal(commandPolicy(h.runtime.store,target,await rootIdentity(target)),'sandboxed');
  assert.equal(await readFile(join(target,'source.txt'),'utf8'),'committed\n');await assert.rejects(readFile(join(target,'untracked.txt')),{code:'ENOENT'});
  for(let i=0;i<2;i++) {
    const submitted=await h.runtime.reply({protocol_version:6,op:'submit',thread_id:id,request_id:`run${i}`,text:'hello',approval:'auto'});assert.equal(submitted.ok,true,JSON.stringify(submitted));
    if(harness==='codex') {await until(()=>h.runtime.snapshot(id).pending_approval!==null); const approval=h.runtime.snapshot(id).pending_approval!;await h.runtime.reply({protocol_version:6,op:'approve',thread_id:id,approval_id:approval.id,allow:true});}
    await until(()=>!h.runtime.sessions.has(id));assert.equal(h.runtime.snapshot(id).status,'idle',JSON.stringify(h.runtime.snapshot(id).error));
  }
  if(harness==='pi') {assert.equal(contexts.length,2); for(const context of contexts) {assert.match(context,/Plait run context/);assert.ok(context.includes(target));} assert.equal(await readFile(join(target,'pi-owned.txt'),'utf8'),'isolated'); await assert.rejects(readFile(join(h.root,'pi-owned.txt')),{code:'ENOENT'});assert.match(contexts[0],/Plait run context/);assert.ok(contexts[0].includes(target));assert.match(contexts[0],/writer/);}
  if(harness==='codex') {
    assert.equal(await readFile(join(target,'codex-run0.txt'),'utf8'),'turn-1\n'); await assert.rejects(readFile(join(h.root,'codex-run0.txt')),{code:'ENOENT'});
    const frames=(await readFile(join(h.store,'frames.jsonl'),'utf8')).trim().split('\n').map(line=>JSON.parse(line));
    for(const method of ['thread/start','thread/resume']) {const frame=frames.find(f=>f.method===method);assert.equal(frame.params.cwd,target);assert.match(frame.params.developerInstructions,/Plait run context/);assert.equal(frame.params.sandbox,'workspace-write');}
  }
  if(harness==='claude') {
    const rows=(await readFile(join(h.store,'claude-images.jsonl'),'utf8')).trim().split('\n').map(line=>JSON.parse(line));
    const starts=rows.flatMap(row=>row.stdin?[JSON.parse(row.stdin)]:[]).filter(frame=>frame.request?.subtype==='initialize' && typeof frame.request.appendSystemPrompt==='string');assert.equal(starts.length,2);
    for(const frame of starts) {assert.match(frame.request.appendSystemPrompt,/Plait run context/);assert.ok(frame.request.appendSystemPrompt.includes(target));assert.equal(frame.request.systemPromptSnapshot,false);}
    const used=rows.filter(row=>row.observed==='unasked');assert.equal(used.length,2);for(const row of used) assert.equal(row.mode,'auto');
    const runs=rows.filter(row=>row.argv?.includes('stream-json') && row.argv.some((arg:string)=>arg==='--permission-mode=auto'));assert.equal(runs.length,2); const created=runs[0].argv.find((arg:string)=>arg.startsWith('--session-id=')).slice('--session-id='.length);assert.ok(runs[1].argv.includes(`--resume=${created}`));assert.equal(await readFile(join(target,'claude-1.txt'),'utf8'),'turn 1\n');await assert.rejects(readFile(join(h.root,'claude-1.txt')),{code:'ENOENT'});
  }
  assert.equal(await readFile(join(h.root,'untracked.txt'),'utf8'),'keep');
});
test('preparation cancellation starts no harness and releases the checkout',async t=>{
  const h=await pi(t,[fauxAssistantMessage('done')]);let release!:()=>void;const held=new Promise<void>(r=>release=r);
  const original=h.runtime.records.before.bind(h.runtime.records);let first=true;let beforeCalls=0;
  h.runtime.records.before=async(...args)=>{beforeCalls++;if(first){first=false;await held;}return original(...args);};
  const ids:string[]=[];
  try {
    for(let i=0;i<2;i++){const root=i?h.root+'2':h.root;if(i){await mkdir(root);await exec('/usr/bin/git',['init','-q',root]);}
      const r:any=await h.runtime.reply({protocol_version:6,op:'create_thread',checkout:root,request_id:`c${i}`,harness:'pi',provider:'faux',model_id:'faux-1'});ids.push(r.result.thread.id);
      assert.equal((await h.runtime.reply({protocol_version:6,op:'submit',thread_id:ids[i],request_id:`s${i}`,text:'hi',approval:'auto'})).ok,true);}
    await until(()=>h.runtime.capacity.phases.get(ids[1])==='resource_wait');
    await h.runtime.reply({protocol_version:6,op:'stop',thread_id:ids[1]});await until(()=>!h.runtime.sessions.has(ids[1]));
    assert.equal(beforeCalls,1);assert.equal(h.runtime.snapshot(ids[1]).status,'stopped');const lease=await acquireCheckout(h.root+'2');await lease.close();
  }finally{release();await until(()=>h.runtime.active.size===0);}
});
for(const code of ['EAGAIN','EMFILE','ENOSPC']) test(`resource ${code} fails explicitly and releases writer reservation`,async t=>{
  const h=await codex(t);const thread=await h.create();
  h.runtime.codex.acquire=async()=>{throw Object.assign(new Error('fixture resource failure'),{code});};
  assert.equal((await h.runtime.reply({protocol_version:6,op:'submit',thread_id:thread.id,request_id:'run',text:'hello',approval:'auto'})).ok,true);
  await until(()=>!h.runtime.sessions.has(thread.id));assert.equal(h.runtime.snapshot(thread.id).error?.code,'resource_exhausted');assert.equal(h.runtime.active.size,0);const lease=await acquireCheckout(h.root);await lease.close();
});
test('failed before record never opens a harness',async t=>{
  const h=await codex(t);const thread=await h.create();await writeFile(join(h.store,'records'),'blocked');
  assert.equal((await h.runtime.reply({protocol_version:6,op:'submit',thread_id:thread.id,request_id:'run',text:'hello',approval:'auto'})).ok,true);
  await until(()=>!h.runtime.sessions.has(thread.id));assert.equal(h.runtime.snapshot(thread.id).error?.code,'record_failed');assert.equal(await h.turns(),0);const lease=await acquireCheckout(h.root);await lease.close();
});

test('lost reply and restart resume the same admission; linked continuation leaves source history intact',async t=>{
  const h=await codex(t);await chmod(h.store,0o700);await commit(h.root);const source=await h.create();
  const request={...start(h.root,'linked','codex'),placement:'worktree',source_thread_id:source.id};
  assert.equal((await h.runtime.reply(request)).ok,true);const first=await admitted(h.runtime,'linked');
  const sourceBefore=structuredClone(h.runtime.snapshot(source.id));
  // A crash between child completion and the admission receipt update.
  h.runtime.store.db.prepare('UPDATE thread_admissions SET result=? WHERE request_id=?').run(JSON.stringify({...first,state:'preparing',thread_id:null,worktree:null,checkout:h.root}),'linked');
  await h.close();const next=await h.reopen();const recovered=await admitted(next.runtime,'linked');assert.equal(recovered.thread_id,first.thread_id);
  const origin:any=await next.runtime.reply({op:'thread_origin',thread_id:first.thread_id});assert.equal(origin.result.admission.source_thread_id,source.id);
  assert.deepEqual(next.runtime.snapshot(source.id),sourceBefore);
  assert.equal(next.runtime.store.db.prepare('SELECT count(*) AS n FROM worktree_operations').get()!.n,1);
  assert.equal((await next.runtime.reply(request) as any).result.admission.thread_id,first.thread_id);
  assert.equal((await next.runtime.reply({...request,request_id:'forged',checkout:first.checkout})).error?.code,'permission_denied');
});

test('resource syscall failures before acceptance release descriptors and do not record an input',async t=>{
  const childProcess=await import('node:child_process');const fs=(await import('node:fs/promises')).default;const {syncBuiltinESMExports}=await import('node:module');
  const h=await codex(t);const thread=await h.create();
  for(const code of ['EAGAIN','EMFILE'] as const) {
    const spawn=childProcess.default.spawn;const open=fs.open;
    try {
      if(code==='EAGAIN') childProcess.default.spawn=((...args:Parameters<typeof spawn>)=>{ if((args[1] as string[])[0]==='lock-fd') throw Object.assign(new Error('fixture process failure'),{code});return spawn(...args); }) as typeof spawn;
      else fs.open=(async(...args:Parameters<typeof open>)=>{if(String(args[0])===h.root)throw Object.assign(new Error('fixture descriptor failure'),{code});return open(...args);}) as typeof open;
      syncBuiltinESMExports();
      const reply=await h.runtime.reply({protocol_version:6,op:'submit',thread_id:thread.id,request_id:code,text:'not accepted',approval:'auto'});
      assert.equal(reply.ok,false);assert.equal(reply.error?.code,'resource_exhausted');assert.equal(h.runtime.active.size,0);assert.equal(h.runtime.store.inputs(thread.id).length,0);
    }finally{childProcess.default.spawn=spawn;fs.open=open;syncBuiltinESMExports();}
    const lease=await acquireCheckout(h.root);await lease.close();
  }
});

test('project authorization survives restart but never follows a replaced repository',async t=>{
  const {rename}=await import('node:fs/promises');const h=await codex(t);await chmod(h.store,0o700);await commit(h.root);
  const info:any=await h.runtime.reply({op:'admission_info',checkout:h.root});
  assert.equal((await h.runtime.reply({op:'set_project_isolation',checkout:h.root,identity:info.result.project_identity,auto_worktree:true})).ok,true);
  await h.close();const next=await h.reopen();assert.equal((await next.runtime.reply({op:'admission_info',checkout:h.root}) as any).result.auto_worktree,true);
  await rename(h.root,h.root+'-original');await mkdir(h.root);await exec('/usr/bin/git',['init','-q',h.root]);await commit(h.root);
  const replacement:any=await next.runtime.reply({op:'admission_info',checkout:h.root});assert.equal(replacement.result.auto_worktree,false);
  assert.equal((await next.runtime.reply({op:'set_project_isolation',checkout:h.root,identity:info.result.project_identity,auto_worktree:true})).error?.code,'permission_denied');
});


test('a checkout replaced during model discovery cannot receive an admitted thread',async t=>{
  const {rename}=await import('node:fs/promises');const h=await codex(t);await commit(h.root);
  const model=h.runtime.codex.model.bind(h.runtime.codex);
  h.runtime.codex.model=async id=>{
    const result=await model(id);
    await rename(h.root,h.root+'-original');await mkdir(h.root);await exec('/usr/bin/git',['init','-q',h.root]);
    return result;
  };
  assert.equal((await h.runtime.reply(start(h.root,'replaced-during-discovery','codex'))).ok,true);
  let result:Admission|undefined;
  await until(async()=>{const reply:any=await h.runtime.reply({op:'admission_status',request_id:'replaced-during-discovery'});assert.equal(reply.ok,true);result=reply.result.admission;return result!.state!=='preparing';});
  assert.equal(result!.state,'failed');assert.equal(result!.error?.code,'permission_denied');assert.match(result!.error!.message,/identity changed/);
  assert.equal(h.runtime.store.list(h.root).length,0);assert.equal(await h.turns(),0);
});


test('held cleanup refuses busy and automatic worktree placement without changing protected state',async t=>{
  const {cleanupHelper}=await import('../src/cleanup.ts');const h=await codex(t);await chmod(h.store,0o700);await commit(h.root);
  const info:any=await h.runtime.reply({op:'admission_info',checkout:h.root});
  assert.equal((await h.runtime.reply({op:'set_project_isolation',checkout:h.root,identity:info.result.project_identity,auto_worktree:true})).ok,true);
  const lease=await acquireCheckout(h.root);const token='admission-held';
  await cleanupHelper('begin',h.root,token,'A process still owns this checkout.',lease);
  await cleanupHelper('mark',h.root,token,'A process still owns this checkout.',lease);
  await lease.close();
  try {
    for(const busy of [false,true]) {
      if(busy) h.runtime.active.set(h.root,'finishing');
      for(const placement of ['project','worktree']) {
        const reply=await h.runtime.reply({...start(h.root,`held-${busy}-${placement}`,'codex'),placement});
        assert.equal(reply.ok,false);assert.equal(reply.error?.code,'cleanup_uncertain');assert.match(reply.error!.message,/A process still owns this checkout/);
      }
      h.runtime.active.delete(h.root);
    }
    assert.equal(h.runtime.store.db.prepare('SELECT count(*) AS n FROM thread_admissions').get()!.n,0);
    assert.equal(h.runtime.store.db.prepare('SELECT count(*) AS n FROM worktree_operations').get()!.n,0);
    assert.equal((await cleanupHelper('read',h.root)).cleanup?.token,token);
    assert.equal(await readFile(join(h.root,'source.txt'),'utf8'),'committed\n');
  }finally{h.runtime.active.delete(h.root);await cleanupHelper('resolve',h.root,token);}
  assert.equal((await h.runtime.reply({...start(h.root,'held-false-worktree','codex'),placement:'worktree'})).ok,true);
  assert.ok((await admitted(h.runtime,'held-false-worktree')).worktree);
});

test('placement probes share one lease check and a Git write is not a busy project',async t=>{
  const h=await codex(t);await chmod(h.store,0o700);await commit(h.root);
  // The lease directory's first use is outside this test.
  assert.equal((await h.runtime.reply({op:'admission_info',checkout:h.root}) as any).result.busy,false);
  for(let i=0;i<5;i++) {
    const infos:any[]=await Promise.all([1,2,3].map(()=>h.runtime.reply({op:'admission_info',checkout:h.root})));
    assert.deepEqual(infos.map(info=>info.result?.busy ?? info.error),[false,false,false]);
  }
  const info:any=await h.runtime.reply({op:'admission_info',checkout:h.root});
  assert.equal((await h.runtime.reply({op:'set_project_isolation',checkout:h.root,identity:info.result.project_identity,auto_worktree:true})).ok,true);
  // Stage, commit, push, fetch and pull hold an exclusive lease; none of them is a run.
  const write=await acquireCheckout(h.root,'exclusive');
  try {
    assert.equal((await h.runtime.reply({op:'admission_info',checkout:h.root}) as any).result.busy,false);
    assert.equal((await h.runtime.reply(start(h.root,'during-git-write','codex'))).ok,true);
  } finally {await write.close();}
  const created=await admitted(h.runtime,'during-git-write');
  assert.equal(created.worktree,null);assert.equal(created.checkout,h.root);
  assert.equal(h.runtime.store.db.prepare('SELECT count(*) AS n FROM worktree_operations').get()!.n,0);
  // Another host's run is still a busy project.
  const run=await acquireCheckout(h.root);
  try {assert.equal((await h.runtime.reply({op:'admission_info',checkout:h.root}) as any).result.busy,true);}
  finally {await run.close();}
});
test('a run start waits for an in-flight placement probe instead of colliding with it',async t=>{
  const h=await pi(t,[fauxAssistantMessage('done')]);await commit(h.root);
  const created:any=await h.runtime.reply({protocol_version:6,op:'create_thread',checkout:h.root,request_id:'c',harness:'pi',provider:'faux',model_id:'faux-1'});
  let release!:()=>void;const held=new Promise<void>(resolve=>release=resolve);let probing=false;
  // The probe's real lease, held long enough that a start would otherwise collide.
  (h.runtime.admissions as any).writerElsewhere=async(root:string)=>{const lease=await acquireCheckout(root);probing=true;await held;await lease.close();return false;};
  const info=h.runtime.reply({op:'admission_info',checkout:h.root});
  await until(()=>probing);
  const submitted=h.runtime.reply({protocol_version:6,op:'submit',thread_id:created.result.thread.id,request_id:'run',text:'hello',approval:'auto'});
  await pause();release();
  const reply:any=await submitted;assert.equal(reply.ok,true,JSON.stringify(reply));
  assert.equal((await info as any).result.busy,false);
  await until(()=>!h.runtime.sessions.has(created.result.thread.id));
  assert.equal(h.runtime.snapshot(created.result.thread.id).status,'idle',JSON.stringify(h.runtime.snapshot(created.result.thread.id).error));
});

test('plain folder threads run Pi without records and promote on the next run after Git initialization', async t => {
  const h = await pi(t, [
    fauxAssistantMessage([fauxToolCall('write', { path: 'created.txt', content: 'hello' }, { id: 'write' })], { stopReason: 'toolUse' }),
    fauxAssistantMessage('done'), fauxAssistantMessage('done'),
  ]);
  const folder = join(h.base, 'plain'); await mkdir(folder);
  const info: any = await h.runtime.reply({ op: 'admission_info', checkout: folder });
  assert.equal(info.result.parent, null); assert.equal(info.result.refusal, 'Worktrees need a Git repository.');
  assert.equal(info.result.auto_worktree, false);
  const isolation = await h.runtime.reply({ op: 'set_project_isolation', checkout: folder, identity: info.result.identity, auto_worktree: true });
  assert.equal(isolation.error?.message, 'Worktrees need a Git repository.');
  const created: any = await h.runtime.reply({ protocol_version: 6, op: 'create_thread', checkout: folder, request_id: 'plain', provider: 'faux', model_id: 'faux-1' });
  assert.equal(created.ok, true, JSON.stringify(created)); const id = created.result.thread.id;
  const run = async (request_id: string) => {
    const reply = await h.runtime.reply({ protocol_version: 6, op: 'submit', thread_id: id, request_id, text: 'go', approval: 'auto' });
    assert.equal(reply.ok, true, JSON.stringify(reply)); await until(() => !h.runtime.sessions.has(id));
    assert.equal(h.runtime.snapshot(id).status, 'idle', JSON.stringify(h.runtime.snapshot(id).error));
  };
  await run('run1'); assert.equal(await readFile(join(folder, 'created.txt'), 'utf8'), 'hello');
  assert.equal(h.runtime.store.db.prepare('SELECT vcs FROM thread_vcs WHERE thread=?').get(id)?.vcs, 'folder');
  assert.equal(h.runtime.store.db.prepare('SELECT count(*) AS n FROM records WHERE thread=?').get(id)?.n, 0);
  const undo = await h.runtime.reply({ protocol_version: 6, op: 'undo_turn', thread_id: id, record_id: 'missing', request_id: 'undo' });
  assert.equal(undo.error?.code, 'unsupported'); assert.equal(undo.error?.message, 'Undo needs a Git repository; this folder is not one.');
  const review: any = await h.runtime.reply({ op: 'turn_review_info', thread_id: id });
  assert.equal(review.result.features.record_diff, false);

  // With a committed base, the next run promotes the thread and records the turn.
  await exec('/usr/bin/git', ['init', '-q', folder]);
  await commit(folder);
  await run('run2');
  assert.equal(h.runtime.store.db.prepare('SELECT vcs FROM thread_vcs WHERE thread=?').get(id)?.vcs, 'git');
  const records = h.runtime.store.db.prepare('SELECT kind FROM records WHERE thread=? AND run_request=? ORDER BY kind').all(id, 'run2');
  assert.deepEqual(records.map(row => row.kind), ['after', 'before']);
});

test('Git initialization during edit approval preserves a dirty tracked file', async t => {
  const h = await pi(t, [
    fauxAssistantMessage([fauxToolCall('edit', { path: 'notes.txt', old_text: 'my edit', new_text: 'overwritten' }, { id: 'edit' })], { stopReason: 'toolUse' }),
    fauxAssistantMessage('done'),
  ]);
  const folder = join(h.base, 'plain');
  const notes = join(folder, 'notes.txt');
  await mkdir(folder);
  await writeFile(notes, 'my edit');
  const created: any = await h.runtime.reply({ protocol_version: 6, op: 'create_thread', checkout: folder, request_id: 'plain', provider: 'faux', model_id: 'faux-1' });
  assert.equal(created.ok, true, JSON.stringify(created));
  const id = created.result.thread.id;
  const submitted = await h.runtime.reply({ protocol_version: 6, op: 'submit', thread_id: id, request_id: 'run', text: 'Edit notes.txt', approval: 'ask' });
  assert.equal(submitted.ok, true, JSON.stringify(submitted));
  await until(() => !!h.runtime.snapshot(id).pending_approval);
  await exec('/usr/bin/git', ['init', '-q', folder]);
  await writeFile(notes, 'base');
  await commit(folder);
  await writeFile(notes, 'my edit');
  const approval = h.runtime.snapshot(id).pending_approval!;
  const approved = await h.runtime.reply({ protocol_version: 6, op: 'approve', thread_id: id, approval_id: approval.id, allow: true });
  assert.equal(approved.ok, true, JSON.stringify(approved));
  await until(() => !h.runtime.sessions.has(id));
  const edit = h.runtime.snapshot(id).tools.find(tool => tool.name === 'edit');
  assert.ok(edit?.output.includes('This folder became a Git repository during the run. Send a new message to continue with Undo protection.'), JSON.stringify(edit));
  assert.equal(await readFile(notes, 'utf8'), 'my edit');
});
