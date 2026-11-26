import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdir, mkdtemp, rename, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider, fauxAssistantMessage } from '@earendil-works/pi-ai/providers/faux';
import { Runtime } from '../src/runtime.ts';
import { queueRequest, CHECKOUT_QUEUE_LIMIT } from '../src/checkout-queue.ts';
import { acquireCheckout, rootIdentity } from '../src/safety.ts';
import { setCommandPolicy } from '../src/command/policy.ts';
import { setup, exec } from './support/safety.ts';
const pause=()=>new Promise(resolve=>setTimeout(resolve,20));
async function until(done:()=>boolean) {const end=Date.now()+30000;while(!done()){assert.ok(Date.now()<end,'Queue did not settle');await pause();}}
class FakeRuntime extends Runtime {override async resources(){return {skills:[],templates:[],problems:[],mcpServers:[],mcpEntries:new Map()};}}
async function fixture(t:Parameters<typeof setup>[0]) {
 const {root,base}=await setup(t);const directory=await mkdtemp(join(tmpdir(),'plait-queue-host-'));
 // Lock and cleanup records are named by a folder's device and inode and are never removed, and Linux hands a removed
 // folder's inode to the next one. Each test keeps its own, so it cannot inherit a hold an earlier test left on purpose.
 const leases=await mkdtemp(join(tmpdir(),'plait-leases-'));process.env.PLAIT_LEASE_DIR=leases;const models=createModels();const faux=fauxProvider({tokensPerSecond:1e8});models.setProvider(faux.provider);
 let runtime=new FakeRuntime(directory,models,new Set(['faux']));
 // A closing runtime can still write a record, so its directory is outside the folder the test removes first and goes last.
 t.after(async()=>{await runtime.close();await rm(directory,{recursive:true,force:true});await rm(leases,{recursive:true,force:true,maxRetries:5,retryDelay:50});});
 return {root,base,faux,get runtime(){return runtime;},restart:async()=>{await runtime.close();runtime=new FakeRuntime(directory,models,new Set(['faux']));}};
}
async function thread(runtime:Runtime,root:string,id:string){const result:any=await runtime.handle({op:'create_thread',protocol_version:6,request_id:id,checkout:root,provider:'faux',model_id:'faux-1'});return result.thread.id as string;}
const queue=(id:string,request:string,text=request)=>({op:'submit_checkout',thread_id:id,request_id:request,text,approval:'auto'});
async function run(runtime:Runtime,id:string,request:string){await runtime.handle({op:'submit',protocol_version:6,thread_id:id,request_id:request,text:request,approval:'auto'});}
function barrier(){let release!:()=>void;const waiting=new Promise<void>(resolve=>release=resolve);return {release,waiting};}
test('queue operations reject unknown fields, invalid IDs and empty input',()=>{
 assert.deepEqual(queueRequest({op:'checkout_queue_info'}),{op:'checkout_queue_info'});
 for(const bad of [{...queue('t','r'),protocol_version:6},{...queue('t','r'),extra:true},{...queue('t','r'),thread_id:'../x'},{...queue('t','r'),text:''},{op:'resume_checkout_queue'},{op:'checkout_queue_info',checkout:'/repo'}])assert.throws(()=>queueRequest(bad));
});
test('twenty waiters drain FIFO while another checkout progresses independently',async t=>{
 const h=await fixture(t),hold=barrier();let began=0;const seen:string[]=[];
 h.faux.setResponses([async()=>{began++;await hold.waiting;return fauxAssistantMessage('first');},...Array.from({length:21},()=>(context: { messages: unknown })=>{seen.push(JSON.stringify(context.messages));return fauxAssistantMessage('done');})]);
 const writer=await thread(h.runtime,h.root,'writer');await run(h.runtime,writer,'hold');await until(()=>began===1);
 try{const ids:string[]=[];for(let i=0;i<20;i++){const id=await thread(h.runtime,h.root,`c${i}`);ids.push(id);await h.runtime.handle(queue(id,`q${i}`));}
 assert.deepEqual(ids.map(id=>h.runtime.store.waiters.summary(id,h.root)!.position),Array.from({length:20},(_,i)=>i+1));
 const other=join(h.base,'other');await mkdir(other);await exec('/usr/bin/git',['init','-q',other]);const id=await thread(h.runtime,other,'other');await h.runtime.handle(queue(id,'independent'));await until(()=>h.runtime.snapshot(id).status==='idle');assert.equal(h.runtime.active.get(h.root),writer);
 hold.release();await until(()=>ids.every(id=>h.runtime.store.waiters.summary(id,h.root)===null)&&h.runtime.active.size===0);
 assert.deepEqual(h.runtime.store.db.prepare("SELECT request FROM checkout_waiters WHERE checkout=? AND state='started' ORDER BY ordering").all(h.root).map(row=>row.request),Array.from({length:20},(_,i)=>`q${i}`));
 assert.equal(seen.length,21);for(let i=0;i<20;i++)assert.match(seen[i+1],new RegExp(`q${i}(?:[^0-9]|$)`));
 }finally{hold.release();}
});
test('follow-ups and checkout waiters share acceptance order without starvation',async t=>{
 const h=await fixture(t),hold=barrier();let began=false;const seen:string[]=[];
 h.faux.setResponses([async()=>{began=true;await hold.waiting;return fauxAssistantMessage('held');},...Array.from({length:3},()=>(context: { messages: unknown })=>{seen.push(JSON.stringify(context.messages));return fauxAssistantMessage('done');})]);
 const a=await thread(h.runtime,h.root,'a'),b=await thread(h.runtime,h.root,'b');await run(h.runtime,a,'start');await until(()=>began);
 try{const follow=(request:string)=>h.runtime.handle({op:'submit',protocol_version:6,thread_id:a,request_id:request,text:request,when_busy:'follow_up',approval:'auto'});
 await follow('before');await h.runtime.handle(queue(b,'between'));await follow('after');assert.deepEqual(h.runtime.store.waiters.line(h.root).map(w=>w.request_id),['before','between','after']);
 hold.release();await until(()=>h.runtime.active.size===0&&h.runtime.store.waiters.line(h.root).length===0);assert.match(seen[0],/before/);assert.match(seen[1],/between/);assert.match(seen[2],/after/);
 }finally{hold.release();}
});
test('cancelled head is skipped and a lost acceptance retry preserves one message and images',async t=>{
 const h=await fixture(t),lease=await acquireCheckout(h.root);h.faux.setResponses([fauxAssistantMessage('done')]);const a=await thread(h.runtime,h.root,'a'),b=await thread(h.runtime,h.root,'b');
 const png=Buffer.alloc(33);Buffer.from([137,80,78,71,13,10,26,10]).copy(png);png.writeUInt32BE(13,8);png.write('IHDR',12);png.writeUInt32BE(1,16);png.writeUInt32BE(1,20);const image=createHash('sha256').update(png).digest('hex');h.runtime.store.images.put(b,image,0,png.length,png.toString('base64'));
 try{await h.runtime.handle(queue(a,'cancel-me'));const request={...queue(b,'only-once'),images:[image]};const accepted=await h.runtime.handle(request);assert.deepEqual(await h.runtime.handle(request),accepted);assert.equal((await h.runtime.reply({...request,approval:'ask'})).error?.code,'conflict');assert.equal(h.runtime.store.images.forRequest(b,'only-once')[0].id,image);
 await h.runtime.handle({op:'cancel_checkout_wait',thread_id:a,request_id:'cancel-me'});assert.equal(h.runtime.store.waiters.get(a,'cancel-me')!.state,'cancelled');assert.equal(h.runtime.store.waiters.summary(b,h.root)!.position,1);await h.runtime.handle(queue(a,'cancel-me'));assert.equal(h.runtime.store.waiters.line(h.root).length,1);
 }finally{await lease.close();}
 await until(()=>h.runtime.store.waiters.get(b,'only-once')?.state==='started'&&h.runtime.active.size===0);assert.equal(h.runtime.snapshot(b).messages.filter(m=>m.id==='request-only-once').length,1);
});
test('restart holds the original line until explicit Resume releases the checkout',async t=>{
 const h=await fixture(t),lease=await acquireCheckout(h.root);const a=await thread(h.runtime,h.root,'a'),b=await thread(h.runtime,h.root,'b');await h.runtime.handle(queue(a,'first'));await h.runtime.handle(queue(b,'second'));await h.restart();await lease.close();
 assert.deepEqual(h.runtime.store.waiters.line(h.root).map(w=>[w.request_id,w.state]),[['first','held'],['second','held']]);await pause();assert.equal(h.runtime.active.size,0);assert.equal(h.runtime.store.inputs(a).length,0);
 h.faux.setResponses([fauxAssistantMessage('one'),fauxAssistantMessage('two')]);await h.runtime.handle({op:'resume_checkout_queue',thread_id:b});await until(()=>h.runtime.store.waiters.line(h.root).length===0&&h.runtime.active.size===0);assert.equal(h.runtime.snapshot(a).status,'idle');assert.equal(h.runtime.snapshot(b).status,'idle');
});
for(const change of ['model','mode','policy','moved','replaced','cancel-race'] as const)test(`head revalidation: ${change} cannot strand later waiters`,async t=>{
 const h=await fixture(t),lease=await acquireCheckout(h.root);h.faux.setResponses([fauxAssistantMessage('next')]);const a=await thread(h.runtime,h.root,'a'),b=await thread(h.runtime,h.root,'b');await h.runtime.handle(queue(a,'head'));await h.runtime.handle(queue(b,'tail'));await until(()=>h.runtime.active.size===0);
 if(change==='model'){const s=h.runtime.store.get(a);s.model.model_id='missing';h.runtime.store.save(s);}
 if(change==='mode'){const s=h.runtime.store.get(a);s.harness={id:'pi',version:null,mode:'changed'};h.runtime.store.save(s);}
 if(change==='policy')setCommandPolicy(h.runtime.store,h.root,await rootIdentity(h.root),'sandboxed');
 if(change==='moved'||change==='replaced'){
 // Retain the original inode: Linux may reuse an unlinked one immediately.
 await rename(h.root,join(h.base,'original'));if(change==='replaced'){await mkdir(h.root);await exec('/usr/bin/git',['init','-q',h.root]);}}
 if(change==='cancel-race')await h.runtime.handle({op:'cancel_checkout_wait',thread_id:a,request_id:'head'});
 await lease.close();await until(()=>h.runtime.store.waiters.line(h.root).length===0&&h.runtime.active.size===0);assert.equal(h.runtime.store.waiters.get(a,'head')!.state,change==='cancel-race'?'cancelled':'failed');assert.equal(h.runtime.store.waiters.get(b,'tail')!.state,['policy','moved','replaced'].includes(change)?'failed':'started');assert.equal(h.runtime.store.db.prepare('SELECT COUNT(*) AS n FROM records WHERE thread=?').get(a)!.n,0);
});
test('full queue and payload limit reject plainly without recording acceptance',async t=>{
 const h=await fixture(t),lease=await acquireCheckout(h.root);const id=await thread(h.runtime,h.root,'a');
 try{for(let i=0;i<CHECKOUT_QUEUE_LIMIT;i++)await h.runtime.handle(queue(id,`q${i}`));assert.equal((await h.runtime.reply(queue(id,'overflow'))).error?.code,'queue_full');assert.equal(h.runtime.store.duplicate(id,'overflow','overflow'),undefined);
 h.runtime.store.db.prepare("UPDATE checkout_waiters SET bytes=? WHERE request='q0'").run(32*1024*1024);await h.runtime.handle({op:'cancel_checkout_wait',thread_id:id,request_id:'q31'});assert.match((await h.runtime.reply(queue(id,'payload'))).error!.message,/32 MiB/);
 for(let i=0;i<31;i++)await h.runtime.handle({op:'cancel_checkout_wait',thread_id:id,request_id:`q${i}`});
 }finally{await lease.close();}
});
test('Cancel after admission uses Stop and an old cancellation cannot stop a later run',async t=>{
 const h=await fixture(t),first=barrier(),second=barrier();let entered=0;
 h.faux.setResponses([async()=>{entered++;await first.waiting;return fauxAssistantMessage('one');},async()=>{entered++;await second.waiting;return fauxAssistantMessage('two');}]);
 const id=await thread(h.runtime,h.root,'thread');
 try{await h.runtime.handle(queue(id,'first'));await until(()=>entered===1);
 await h.runtime.handle({op:'cancel_checkout_wait',thread_id:id,request_id:'first'});assert.equal(h.runtime.snapshot(id).status,'stopped');first.release();await until(()=>h.runtime.active.size===0);
 await run(h.runtime,id,'later');await until(()=>entered===2);await h.runtime.handle({op:'cancel_checkout_wait',thread_id:id,request_id:'first'});assert.equal(h.runtime.snapshot(id).status,'running');
 }finally{first.release();second.release();}
 await until(()=>h.runtime.active.size===0);
});
test('a stale idle summary cannot accept a busy send; explicit Wait owns the untouched request',async t=>{
 const h=await fixture(t),id=await thread(h.runtime,h.root,'thread');
 const summary:any=await h.runtime.handle({op:'watch_threads_queue'});assert.equal(summary.threads[0].status,'idle');
 const lease=await acquireCheckout(h.root);
 try{
 const refused=await h.runtime.reply({op:'submit',protocol_version:6,thread_id:id,request_id:'message',text:'message'});
 assert.equal(refused.error?.code,'busy');assert.equal(h.runtime.store.duplicate(id,'message','message'),undefined);
 await h.runtime.handle(queue(id,'message'));assert.equal(h.runtime.store.waiters.get(id,'message')!.state,'waiting');
 await h.runtime.handle({op:'cancel_checkout_wait',thread_id:id,request_id:'message'});
 }finally{await lease.close();}
});
test('receipt bounds retain the oldest pending input and an old cancellation result',async t=>{
 const h=await fixture(t),lease=await acquireCheckout(h.root),id=await thread(h.runtime,h.root,'thread');
 try{
 await h.runtime.handle(queue(id,'oldest'));
 for(let i=0;i<35;i++){await h.runtime.handle(queue(id,`q${i}`));await h.runtime.handle({op:'cancel_checkout_wait',thread_id:id,request_id:`q${i}`});}
 const reply:any=await h.runtime.handle({op:'checkout_queue',thread_id:id});assert.equal(reply.waiters.length,32);assert.equal(reply.waiters[0].request_id,'oldest');
 const cancelled:any=await h.runtime.handle({op:'cancel_checkout_wait',thread_id:id,request_id:'q0'});assert.ok(cancelled.waiters.some((w:any)=>w.request_id==='q0'&&w.state==='cancelled'));
 await h.runtime.handle({op:'cancel_checkout_wait',thread_id:id,request_id:'oldest'});
 }finally{await lease.close();}
});
test('Resume revalidates a moved checkout and finishes its held line with reasons',async t=>{
 const h=await fixture(t),lease=await acquireCheckout(h.root),a=await thread(h.runtime,h.root,'a'),b=await thread(h.runtime,h.root,'b');
 await h.runtime.handle(queue(a,'first'));await h.runtime.handle(queue(b,'second'));await h.restart();await lease.close();
 await rename(h.root,join(h.base,'moved'));
 await h.runtime.handle({op:'resume_checkout_queue',thread_id:a});
 await until(()=>h.runtime.store.waiters.line(h.root).length===0);
 for(const [id,request] of [[a,'first'],[b,'second']])assert.equal(h.runtime.store.waiters.get(id,request)!.state,'failed');
});
test('a host stop during admission holds the waiter instead of failing it',async t=>{
 const h=await fixture(t),a=await thread(h.runtime,h.root,'a');
 // The checkout is free, so the drain is already admitting when the host stops.
 await h.runtime.handle(queue(a,'about-to-start'));await h.restart();
 assert.equal(h.runtime.store.waiters.get(a,'about-to-start')!.state,'held');
 assert.equal(h.runtime.snapshot(a).status,'idle');assert.equal(h.runtime.snapshot(a).error,null);
 h.faux.setResponses([fauxAssistantMessage('ran once')]);await h.runtime.handle({op:'resume_checkout_queue',thread_id:a});
 await until(()=>h.runtime.store.waiters.get(a,'about-to-start')!.state==='started'&&h.runtime.active.size===0);
 assert.equal(h.runtime.snapshot(a).messages.filter(m=>m.id==='request-about-to-start').length,1);
});
test('an unconfirmed cleanup holds the line instead of failing every waiter',async t=>{
 const {cleanupHelper}=await import('../src/cleanup.ts');const h=await fixture(t);
 const a=await thread(h.runtime,h.root,'a'),b=await thread(h.runtime,h.root,'b');
 const lease=await acquireCheckout(h.root);await h.runtime.handle(queue(a,'first'));await h.runtime.handle(queue(b,'second'));
 await cleanupHelper('begin',h.root,'held-cleanup','A process still owns this checkout.',lease);
 await cleanupHelper('mark',h.root,'held-cleanup','A process still owns this checkout.',lease);
 await lease.close();
 // The shared contention timer retries after the lease frees, then meets the cleanup marker.
 await until(()=>h.runtime.store.waiters.get(a,'first')!.state!=='waiting');
 assert.deepEqual(h.runtime.store.waiters.line(h.root).map(w=>[w.request_id,w.state]),[['first','held'],['second','held']]);
 assert.equal(h.runtime.snapshot(a).error?.code,'cleanup_uncertain');assert.match(h.runtime.snapshot(a).error!.message,/A process still owns this checkout/);
 assert.equal(h.runtime.store.inputs(a).length,0);assert.equal(h.runtime.active.size,0);
 for(const [id,request] of [[a,'first'],[b,'second']])await h.runtime.handle({op:'cancel_checkout_wait',thread_id:id,request_id:request});
});
test('Resume releases checkout waiters, never another thread held follow-ups',async t=>{
 const h=await fixture(t),hold=barrier();let began=false;
 h.faux.setResponses([async()=>{began=true;await hold.waiting;throw new Error('run failed');},fauxAssistantMessage('waiter ran')]);
 const a=await thread(h.runtime,h.root,'a'),b=await thread(h.runtime,h.root,'b');
 await run(h.runtime,b,'b-run');await until(()=>began);
 await h.runtime.handle({op:'submit',protocol_version:6,thread_id:b,request_id:'b-follow',text:'b-follow',when_busy:'follow_up',approval:'auto'});
 hold.release();await until(()=>h.runtime.active.size===0);assert.equal(h.runtime.store.waiters.get(b,'b-follow')!.state,'held');
 const lease=await acquireCheckout(h.root);await h.runtime.handle(queue(a,'a-wait'));await h.restart();await lease.close();
 await h.runtime.handle({op:'resume_checkout_queue',thread_id:a});
 await until(()=>h.runtime.store.waiters.get(a,'a-wait')!.state==='started'&&h.runtime.active.size===0);
 // B's follow-up waits for B's own next send, as it did before the checkout queue existed.
 assert.equal(h.runtime.store.waiters.get(b,'b-follow')!.state,'held');assert.equal(h.runtime.snapshot(b).queued!.length,1);
 await h.runtime.handle({op:'cancel_queued',protocol_version:6,thread_id:b,request_id:'b-follow'});
});
test('a summary page reads each checkout line once, not once per thread',async t=>{
 const h=await fixture(t),lease=await acquireCheckout(h.root);const ids:string[]=[];
 for(let i=0;i<40;i++)ids.push(await thread(h.runtime,h.root,`t${i}`));for(let i=0;i<5;i++)await h.runtime.handle(queue(ids[i],`q${i}`));
 const waiters=h.runtime.store.waiters,line=waiters.line.bind(waiters);let reads=0;waiters.line=(checkout:string)=>{reads++;return line(checkout);};
 try{
 const page:any=await h.runtime.handle({op:'list_threads_queue',limit:200,archived:true});
 assert.equal(reads,1);assert.equal(page.threads.length,40);
 assert.deepEqual(ids.slice(0,5).map(id=>page.threads.find((s:any)=>s.id===id).checkout_queue.position),[1,2,3,4,5]);
 assert.equal(page.threads.filter((s:any)=>s.checkout_queue).length,5);
 }finally{waiters.line=line;for(let i=0;i<5;i++)await h.runtime.handle({op:'cancel_checkout_wait',thread_id:ids[i],request_id:`q${i}`});await lease.close();}
});
test('cancelling a follow-up refreshes other waiters positions',async t=>{
 const h=await fixture(t),hold=barrier();let began=false;
 h.faux.setResponses([async()=>{began=true;await hold.waiting;return fauxAssistantMessage('held');},fauxAssistantMessage('b ran')]);
 const a=await thread(h.runtime,h.root,'a'),b=await thread(h.runtime,h.root,'b');await run(h.runtime,a,'a-run');await until(()=>began);
 try{
 await h.runtime.handle({op:'submit',protocol_version:6,thread_id:a,request_id:'a-follow',text:'a-follow',when_busy:'follow_up',approval:'auto'});
 await h.runtime.handle(queue(b,'b-wait'));assert.equal(h.runtime.store.waiters.summary(b,h.root)!.position,2);
 let summaries=0;const stop=h.runtime.subscribe(id=>{if(id==='#')summaries++;});
 await h.runtime.handle({op:'cancel_queued',protocol_version:6,thread_id:a,request_id:'a-follow'});stop();
 // Thread B's summary moved from second to first; the shared stream must hear of it.
 assert.equal(h.runtime.store.waiters.summary(b,h.root)!.position,1);assert.ok(summaries>0);
 }finally{hold.release();}
 await until(()=>h.runtime.active.size===0&&h.runtime.store.waiters.line(h.root).length===0);
});

test('enqueue refuses changed command policy or harness mode without recording a submission', async t => {
  const previous = process.env.PLAIT_GRAPH;
  process.env.PLAIT_GRAPH = '1';
  t.after(() => { if (previous === undefined) delete process.env.PLAIT_GRAPH; else process.env.PLAIT_GRAPH = previous; });
  const h = await fixture(t);
  const id = await thread(h.runtime, h.root, 'destination');
  for (const expect of [{ command_policy: 'sandboxed' }, { mode: 'plan' }]) {
    const reply = await h.runtime.reply({ ...queue(id, 'guarded'), expect });
    assert.equal(reply.error?.code, 'queue_settings_changed');
    assert.match(reply.error!.message, new RegExp(Object.keys(expect)[0]));
    assert.equal(h.runtime.store.waiters.get(id, 'guarded'), undefined);
  }
});

test('submit_checkout accepts expect only with a background feature enabled', t => {
  const env = { ...process.env };
  delete env.PLAIT_AUTOMATIONS;
  delete env.PLAIT_GRAPH;
  const previous = process.env;
  process.env = env;
  t.after(() => { process.env = previous; });
  const request = { ...queue('thread', 'request'), expect: {} };
  assert.throws(() => queueRequest(request), { code: 'invalid_request', message: 'Unknown checkout queue field.' });
  for (const flag of ['PLAIT_AUTOMATIONS', 'PLAIT_GRAPH']) {
    env[flag] = '1';
    assert.deepEqual(queueRequest(request), request);
    delete env[flag];
  }
});

test('a background snapshot reads a Codex thread while its catalog refresh is pending', async t => {
  const h = await fixture(t);
  const id = await thread(h.runtime, h.root, 'codex-read');
  const snapshot = h.runtime.store.get(id);
  snapshot.harness = { id: 'codex', mode: 'default', version: null };
  h.runtime.store.save(snapshot);
  const read = { op: 'stored_thread', protocol_version: 6, thread_id: id };
  assert.equal((await h.runtime.reply(read)).error?.code, 'invalid_request');
  const previous = process.env.PLAIT_AUTOMATIONS;
  process.env.PLAIT_AUTOMATIONS = '1';
  try { h.runtime.startBackground(request => h.runtime.handle(request)); }
  finally { if (previous === undefined) delete process.env.PLAIT_AUTOMATIONS; else process.env.PLAIT_AUTOMATIONS = previous; }

  const pending = barrier();
  const entry = t.mock.method(h.runtime.codex, 'entry', () => pending.waiting);
  const foreground = h.runtime.handle({ ...read, op: 'watch' });
  try {
    assert.equal(entry.mock.callCount(), 1);
    const background = h.runtime.handle(read);
    assert.equal(entry.mock.callCount(), 1);
    const result = await background as { thread: { id: string } };
    assert.equal(result.thread.id, id);
  } finally { pending.release(); await foreground; }
});

test('submit_checkout keeps the pre-identity snapshot until a background feature is active', async t => {
  const h = await fixture(t);
  const id = await thread(h.runtime, h.root, 'title-race');
  const lease = await acquireCheckout(h.root);
  const refresh = h.runtime.store.refreshRootIdentity.bind(h.runtime.store);
  t.mock.method(h.runtime.store, 'refreshRootIdentity', async (threadId: string, checkout: string) => {
    const identity = await refresh(threadId, checkout);
    const snapshot = h.runtime.store.get(threadId);
    snapshot.title = 'Generated title';
    h.runtime.store.save(snapshot);
    return identity;
  });
  try {
    await h.runtime.handle(queue(id, 'first', 'Fallback title'));
    assert.equal(h.runtime.store.get(id).title, 'Fallback title');
    const previous = process.env.PLAIT_AUTOMATIONS;
    process.env.PLAIT_AUTOMATIONS = '1';
    try { h.runtime.startBackground(request => h.runtime.handle(request)); }
    finally { if (previous === undefined) delete process.env.PLAIT_AUTOMATIONS; else process.env.PLAIT_AUTOMATIONS = previous; }
    await h.runtime.handle(queue(id, 'second'));
    assert.equal(h.runtime.store.get(id).title, 'Generated title');
  } finally { await lease.close(); }
});
