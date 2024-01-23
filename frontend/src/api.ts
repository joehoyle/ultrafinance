import { Account } from '../../bindings/Account';
import { UpdateAccount } from '../../bindings/UpdateAccount';
import { Function } from '../../bindings/Function';
import { UpdateFunction } from '../../bindings/UpdateFunction';
import { CreateFunction } from '../../bindings/CreateFunction';
import type { Institution } from '../../bindings/Institution';
import type { Requisition } from '../../bindings/Requisition';
import { TransactionWithMerchant } from '../../bindings/TransactionWithMerchant';
import { Transaction } from '../../bindings/Transaction';
import { Trigger } from '../../bindings/Trigger';
import { User } from '../../bindings/User';
import { UpdateUser } from '../../bindings/UpdateUser';
import { TriggerQueue } from '../../bindings/TriggerQueue';
import { CreateTrigger } from '../../bindings/CreateTrigger';
import { TriggerLog } from '../../bindings/TriggerLog';
import { CreateSession } from '../../bindings/CreateSession';
import { NewUser } from '../../bindings/NewUser';
import { TestFunction } from '../../bindings/TestFunction';
import { UpdateTrigger } from '../../bindings/UpdateTrigger';

const baseUrl = `${import.meta.env.VITE_BACKEND_URL}/api/v1`;

export async function getInstitutions(): Promise<Institution[]> {
	return await request( '/requisitions/institutions' ) as Institution[];
}

export async function getAccounts(): Promise<Account[]> {
	return await request( '/accounts' ) as Account[];
}

export async function getAccount( accountId: number ): Promise<Account> {
	return await request( `/accounts/${accountId}` ) as Account;
}

export async function updateAccount( accountId: number, account: UpdateAccount ): Promise<Account> {
	return await request( `/accounts/${accountId}`, "POST", account ) as Account;
}

export async function deleteAccount( accountId: number ): Promise<undefined> {
	await request( `/accounts/${accountId}`, "DELETE" );
	return undefined;
}

export async function relinkAccount( accountId: number, requisitionId: string ): Promise<Account> {
	return await request( `/accounts/${accountId}/relink`, "POST", {
		requisition_id: requisitionId,
	} ) as Account;
}

export async function getMe(): Promise<User> {
	return await request( `/users/me` ) as User;
}

export async function createUser( user: NewUser ): Promise<User> {
	return await request( `/users`, 'POST', user ) as User;
}

export async function updateMe( user: UpdateUser ): Promise<User> {
	return await request( `/users/me`, "POST", user ) as User;
}

export async function getTransaction( transactionId: number): Promise<TransactionWithMerchant> {
	return await request( `/transactions/${ transactionId }` ) as TransactionWithMerchant;
}

export async function deleteTransaction( transactionId: number): Promise<null> {
	return await request( `/transactions/${ transactionId }`, 'DELETE' ) as null;
}

export async function getTransactions(search?: string, page?: number, per_page?: number): Promise<TransactionWithMerchant[]> {
	const args: Record<string, string | number> = {};
	if ( search ) {
		args['search'] = search;
	}
	if ( page ) {
		args['page'] = page;
	}
	if ( per_page ) {
		args['per_page'] = per_page;
	}

	return await request( '/transactions', 'GET', args ) as TransactionWithMerchant[];
}

export async function getFunctions(): Promise<Function[]> {
	return await request( '/functions' ) as Function[];
}

export async function getFunction( functionId: number ): Promise<Function> {
	return await request( `/functions/${ functionId }` ) as Function;
}

export async function updateFunction( functionId: number, _function: UpdateFunction ): Promise<Function> {
	return await request( `/functions/${functionId}`, "POST", _function ) as Function;
}

export async function createFunction( _function: CreateFunction ): Promise<Function> {
	return await request( `/functions`, "POST", _function ) as Function;
}

export async function deleteFunction( functionId: number ): Promise<null> {
	return await request( `/functions/${functionId}`, "DELETE" ) as null;
}

export async function testFunction( functionId: number, data: TestFunction ): Promise<string> {
	return await request( `/functions/${functionId}/test`, "POST", data ) as string;
}

export async function getTriggers(): Promise<Trigger[]> {
	return await request( '/triggers' ) as Trigger[];
}

export async function getTrigger( triggerId: number ): Promise<Trigger> {
	return await request( `/triggers/${ triggerId }` ) as Trigger;
}

export async function createTrigger( trigger: CreateTrigger ): Promise<Trigger> {
	return await request( `/triggers`, "POST", trigger ) as Trigger;
}

export async function updateTrigger( triggertId: number, trigger: UpdateTrigger ): Promise<Trigger> {
	return await request( `/triggers/${ triggertId }`, "POST", trigger ) as Trigger;
}

export async function deleteTrigger( triggerId: number ): Promise<null> {
	return await request( `/triggers/${triggerId}`, "DELETE" ) as null;
}

export async function getTriggerQueue(): Promise<TriggerQueue[]> {
	return await request( `/triggers/queue` ) as TriggerQueue[];
}

export async function processTriggerQueue(): Promise<TriggerLog[]> {
	return await request( `/triggers/queue/process`, "POST" ) as TriggerLog[];
}

export async function getTriggerLog(): Promise<TriggerLog[]> {
	return await request( `/triggers/log` ) as TriggerLog[];
}

export async function createRequisition(institutionId?: string, accountId?: number): Promise<Requisition> {
	return await request( '/requisitions', 'POST', {
		institution_id: institutionId,
		account_id: accountId,
	} ) as Requisition;
}

export async function createAccounts(requisitionId: string): Promise<Account[]> {
	return await request( '/accounts', 'POST', {
		requisition_id: requisitionId,
	} ) as Account[];
}

export async function syncAccounts(): Promise<Transaction[]> {
	return await request( `/accounts/sync`, 'POST' ) as Transaction[];
}

export async function syncAccount(accountId: number): Promise<Transaction[]> {
	return await request( `/accounts/${ accountId }/sync`, 'POST' ) as Transaction[];
}

export async function createSession(details: CreateSession): Promise<User> {
	return await request( `/users/session`, 'POST', details ) as User;
}

export async function deleteSession(): Promise<null> {
	return await request( `/users/session`, 'DELETE' ) as null;
}

export async function request(url: string, method: 'POST' | 'DELETE' | "GET" = "GET", data: any = {} ) : Promise<unknown> {
	const params: RequestInit = {
		method,
		credentials: "include",
		headers: {
			'Content-Type': 'application/json',
		},
	};

	if ( method === 'POST' ) {
		params.body = JSON.stringify( data );
	} else {
		url += '?' + new URLSearchParams( data ).toString();
	}

	const minTime = wait(300);
	const response = await fetch(`${baseUrl}${ url }`, params );
	await minTime;

	if ( response.ok ) {
		return response.json();
	}

	throw new Error( await response.text() );
}

function wait(delay: number) {
    return new Promise(function(resolve) {
        setTimeout(resolve, delay);
    });
}
