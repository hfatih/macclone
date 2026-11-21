import {invoke} from '@tauri-apps/api/core'
export interface Disk {number:number; model:string; serial:string|null;id:string|null;bytes:number;logical:number;physical:number;bus:string;boot:boolean|null;system:boolean|null;offline:boolean|null;readonly:boolean|null;clustered:boolean|null;health:string|null}
export type Input = {kind:'image';path:string;sector:number}|{kind:'disk';number:number;id:string}
export interface Layout {table:string;filesystem:string;warnings:string[];partitions:{start_lba:number;end_lba:number;kind:string;label:string;filesystem:string}[]}
export interface Status {id:number;revision:number;running:boolean;progress:{phase:string;bytes:number;total:number;missing_sectors:number};message:string|null;report:{outcome:string;sha256:string;verified_bytes:number;missing_sectors:number}|null}
export const api={
 disks:()=>invoke<Disk[]>('list_disks'),
 inspect:(input:Input)=>invoke<Layout>('inspect_disk',{input}),
 plan:(source:Disk,target:Disk)=>invoke<string[]>('plan_clone',{source:source.number,target:target.number,sourceId:source.id,targetId:target.id}),
 status:()=>invoke<Status>('get_operation_status'),
 image:(input:Input,directory:string,block:number,recovery:boolean,resume:boolean)=>invoke<number>('create_image',{request:{input,directory,options:{block,recovery},resume}}),
 verify:(input:Input,expected:string)=>invoke<number>('verify_image',{input,expected}),
 cancel:(id:number)=>invoke<void>('cancel_operation',{id})
}
