/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { Account } from '../models/Account';
import type { CreateAccounts } from '../models/CreateAccounts';
import type { CreateFunction } from '../models/CreateFunction';
import type { CreateRequisition } from '../models/CreateRequisition';
import type { CreateSession } from '../models/CreateSession';
import type { CreateTrigger } from '../models/CreateTrigger';
import type { Function } from '../models/Function';
import type { FunctionWithParams } from '../models/FunctionWithParams';
import type { Institution } from '../models/Institution';
import type { NewUser } from '../models/NewUser';
import type { RelinkAccount } from '../models/RelinkAccount';
import type { Requisition } from '../models/Requisition';
import type { TestFunction } from '../models/TestFunction';
import type { Transaction } from '../models/Transaction';
import type { TransactionWithMerchant } from '../models/TransactionWithMerchant';
import type { Trigger } from '../models/Trigger';
import type { TriggerLog } from '../models/TriggerLog';
import type { TriggerQueue } from '../models/TriggerQueue';
import type { UpdateAccount } from '../models/UpdateAccount';
import type { UpdateFunction } from '../models/UpdateFunction';
import type { UpdateTrigger } from '../models/UpdateTrigger';
import type { UpdateUser } from '../models/UpdateUser';
import type { User } from '../models/User';
import type { CancelablePromise } from '../core/CancelablePromise';
import { OpenAPI } from '../core/OpenAPI';
import { request as __request } from '../core/request';
export class DefaultService {
    /**
     * @returns Account OK
     * @throws ApiError
     */
    public static getApiV1Accounts(): CancelablePromise<Array<Account>> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/accounts',
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param body
     * @returns Account OK
     * @throws ApiError
     */
    public static postApiV1Accounts(
        body: CreateAccounts,
    ): CancelablePromise<Array<Account>> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/accounts',
            body: body,
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @returns TransactionWithMerchant OK
     * @throws ApiError
     */
    public static postApiV1AccountsSync(): CancelablePromise<Record<string, Array<TransactionWithMerchant>>> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/accounts/sync',
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param accountId
     * @returns Account OK
     * @throws ApiError
     */
    public static getApiV1Accounts1(
        accountId: number,
    ): CancelablePromise<Account> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/accounts/{account_id}',
            path: {
                'account_id': accountId,
            },
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param body
     * @param accountId
     * @returns Account OK
     * @throws ApiError
     */
    public static postApiV1Accounts1(
        body: UpdateAccount,
        accountId: number,
    ): CancelablePromise<Account> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/accounts/{account_id}',
            path: {
                'account_id': accountId,
            },
            body: body,
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param accountId
     * @returns any OK
     * @throws ApiError
     */
    public static deleteApiV1Accounts(
        accountId: number,
    ): CancelablePromise<any> {
        return __request(OpenAPI, {
            method: 'DELETE',
            url: '/api/v1/accounts/{account_id}',
            path: {
                'account_id': accountId,
            },
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param body
     * @param accountId
     * @returns Account OK
     * @throws ApiError
     */
    public static postApiV1AccountsRelink(
        body: RelinkAccount,
        accountId: number,
    ): CancelablePromise<Account> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/accounts/{account_id}/relink',
            path: {
                'account_id': accountId,
            },
            body: body,
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param accountId
     * @returns Transaction OK
     * @throws ApiError
     */
    public static postApiV1AccountsSync1(
        accountId: number,
    ): CancelablePromise<Array<Transaction>> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/accounts/{account_id}/sync',
            path: {
                'account_id': accountId,
            },
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @returns FunctionWithParams OK
     * @throws ApiError
     */
    public static getApiV1Functions(): CancelablePromise<Array<FunctionWithParams>> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/functions',
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param body
     * @returns Function OK
     * @throws ApiError
     */
    public static postApiV1Functions(
        body: CreateFunction,
    ): CancelablePromise<Function> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/functions',
            body: body,
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param functionId
     * @returns FunctionWithParams OK
     * @throws ApiError
     */
    public static getApiV1Functions1(
        functionId: number,
    ): CancelablePromise<FunctionWithParams> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/functions/{function_id}',
            path: {
                'function_id': functionId,
            },
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param body
     * @param functionId
     * @returns Function OK
     * @throws ApiError
     */
    public static postApiV1Functions1(
        body: UpdateFunction,
        functionId: number,
    ): CancelablePromise<Function> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/functions/{function_id}',
            path: {
                'function_id': functionId,
            },
            body: body,
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param functionId
     * @returns any OK
     * @throws ApiError
     */
    public static deleteApiV1Functions(
        functionId: number,
    ): CancelablePromise<any> {
        return __request(OpenAPI, {
            method: 'DELETE',
            url: '/api/v1/functions/{function_id}',
            path: {
                'function_id': functionId,
            },
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param body
     * @param functionId
     * @returns string OK
     * @throws ApiError
     */
    public static postApiV1FunctionsTest(
        body: TestFunction,
        functionId: number,
    ): CancelablePromise<string> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/functions/{function_id}/test',
            path: {
                'function_id': functionId,
            },
            body: body,
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param body
     * @returns Requisition OK
     * @throws ApiError
     */
    public static postApiV1Requisitions(
        body: CreateRequisition,
    ): CancelablePromise<Requisition> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/requisitions',
            body: body,
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @returns Institution OK
     * @throws ApiError
     */
    public static getApiV1RequisitionsInstitutions(): CancelablePromise<Array<Institution>> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/requisitions/institutions',
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param page
     * @param perPage
     * @returns TransactionWithMerchant OK
     * @throws ApiError
     */
    public static getApiV1Transactions(
        page?: number,
        perPage?: number,
    ): CancelablePromise<Array<TransactionWithMerchant>> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/transactions',
            query: {
                'page': page,
                'per_page': perPage,
            },
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param transactionId
     * @returns TransactionWithMerchant OK
     * @throws ApiError
     */
    public static getApiV1Transactions1(
        transactionId: number,
    ): CancelablePromise<TransactionWithMerchant> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/transactions/{transaction_id}',
            path: {
                'transaction_id': transactionId,
            },
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param transactionId
     * @returns any OK
     * @throws ApiError
     */
    public static deleteApiV1Transactions(
        transactionId: number,
    ): CancelablePromise<any> {
        return __request(OpenAPI, {
            method: 'DELETE',
            url: '/api/v1/transactions/{transaction_id}',
            path: {
                'transaction_id': transactionId,
            },
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @returns Trigger OK
     * @throws ApiError
     */
    public static getApiV1Triggers(): CancelablePromise<Array<Trigger>> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/triggers',
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param body
     * @returns Trigger OK
     * @throws ApiError
     */
    public static postApiV1Triggers(
        body: CreateTrigger,
    ): CancelablePromise<Trigger> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/triggers',
            body: body,
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @returns TriggerLog OK
     * @throws ApiError
     */
    public static getApiV1TriggersLog(): CancelablePromise<Array<TriggerLog>> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/triggers/log',
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @returns TriggerQueue OK
     * @throws ApiError
     */
    public static getApiV1TriggersQueue(): CancelablePromise<Array<TriggerQueue>> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/triggers/queue',
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @returns TriggerLog OK
     * @throws ApiError
     */
    public static postApiV1TriggersQueueProcess(): CancelablePromise<Record<string, TriggerLog>> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/triggers/queue/process',
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param triggerId
     * @returns Trigger OK
     * @throws ApiError
     */
    public static getApiV1Triggers1(
        triggerId: number,
    ): CancelablePromise<Trigger> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/triggers/{trigger_id}',
            path: {
                'trigger_id': triggerId,
            },
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param body
     * @param triggerId
     * @returns Trigger OK
     * @throws ApiError
     */
    public static postApiV1Triggers1(
        body: UpdateTrigger,
        triggerId: number,
    ): CancelablePromise<Trigger> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/triggers/{trigger_id}',
            path: {
                'trigger_id': triggerId,
            },
            body: body,
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param triggerId
     * @returns any OK
     * @throws ApiError
     */
    public static deleteApiV1Triggers(
        triggerId: number,
    ): CancelablePromise<any> {
        return __request(OpenAPI, {
            method: 'DELETE',
            url: '/api/v1/triggers/{trigger_id}',
            path: {
                'trigger_id': triggerId,
            },
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param body
     * @returns User OK
     * @throws ApiError
     */
    public static postApiV1Users(
        body: NewUser,
    ): CancelablePromise<User> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/users',
            body: body,
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @returns User OK
     * @throws ApiError
     */
    public static getApiV1UsersMe(): CancelablePromise<User> {
        return __request(OpenAPI, {
            method: 'GET',
            url: '/api/v1/users/me',
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param body
     * @returns User OK
     * @throws ApiError
     */
    public static postApiV1UsersMe(
        body: UpdateUser,
    ): CancelablePromise<User> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/users/me',
            body: body,
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @param body
     * @returns User OK
     * @throws ApiError
     */
    public static postApiV1UsersSession(
        body: CreateSession,
    ): CancelablePromise<User> {
        return __request(OpenAPI, {
            method: 'POST',
            url: '/api/v1/users/session',
            body: body,
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
    /**
     * @returns any OK
     * @throws ApiError
     */
    public static deleteApiV1UsersSession(): CancelablePromise<any> {
        return __request(OpenAPI, {
            method: 'DELETE',
            url: '/api/v1/users/session',
            errors: {
                400: `Bad Request`,
                401: `Unauthorized: Can't read session from header`,
                500: `Internal Server Error`,
            },
        });
    }
}
