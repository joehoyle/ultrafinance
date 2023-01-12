import React from 'react'
import ReactDOM from 'react-dom/client'
import './index.css'
import {
	createBrowserRouter,
	RouterProvider,
} from "react-router-dom";

import App, { loader as AppLoader } from './App'
import Accounts, { loader as AccountsLoader } from './routes/accounts';
import Account, { loader as AccountLoader, action as AccountAction } from './routes/account';
import AccountAdd, { loader as AccountAddLoader } from './routes/account-new';
import Transactions, { loader as TransactionsLoader, action as TransactionsAction } from './routes/transactions';
import Transaction, { loader as TransactionLoader, action as TransactionAction } from './routes/transaction';
import AccountsResume, { loader as AccountResumeLoader } from './routes/account-resume';
import Functions, { loader as FunctionsLoader } from './routes/functions';
import Function, { action as FunctionAction, loader as FunctionLoader } from './routes/function';
import FunctionAdd, { action as FunctionAddAction } from './routes/function-new';
import Triggers, { loader as TriggersLoader } from './routes/triggers';
import Trigger, { loader as TriggerLoader, action as TriggerAction } from './routes/trigger';
import TriggerAdd, { loader as TriggerAddLoader, action as TriggerAddAction } from './routes/trigger-new';
import Logs, { loader as LogsLoader } from './routes/logs';
import MyAccount, { action as MyAccountAction } from './routes/my-account';
import Login, { action as LoginAction } from './routes/login';
import Signup, { action as SignupAction } from './routes/signup';
import Homepage from './routes/homepage';

const router = createBrowserRouter([
	{
		path: "/",
		element: <Homepage />,
	},
	{
		element: <App />,
		loader: AppLoader,
		id: "app",
		children: [
			{
				path: '/accounts',
				element: <Accounts />,
				loader: AccountsLoader,
			},
			{
				path: '/accounts/new',
				element: <AccountAdd />,
				loader: AccountAddLoader,
			},
			{
				path: '/accounts/resume',
				element: <AccountsResume />,
				loader: AccountResumeLoader,
			},
			{
				path: '/accounts/:id',
				element: <Account />,
				loader: AccountLoader,
				action: AccountAction,
			},
			{
				path: '/transactions',
				element: <Transactions />,
				loader: TransactionsLoader,
				action: TransactionsAction,
			},
			{
				path: '/transactions/:id',
				element: <Transaction />,
				loader: TransactionLoader,
				action: TransactionAction,
			},
			{
				path: '/functions',
				element: <Functions />,
				loader: FunctionsLoader,
			},
			{
				path: '/functions/new',
				element: <FunctionAdd />,
				action: FunctionAddAction,
			},
			{
				path: '/functions/:id',
				element: <Function />,
				loader: FunctionLoader,
				action: FunctionAction,
			},
			{
				path: '/triggers',
				element: <Triggers />,
				loader: TriggersLoader,
			},
			{
				path: '/triggers/new',
				element: <TriggerAdd />,
				loader: TriggerAddLoader,
				action: TriggerAddAction,
			},
			{
				path: '/triggers/:id',
				element: <Trigger />,
				loader: TriggerLoader,
				action: TriggerAction,
			},
			{
				path: '/logs',
				element: <Logs />,
				loader: LogsLoader,
			},
			{
				path: '/account',
				element: <MyAccount />,
				action: MyAccountAction,
			},
		]
	},
	{
		path: '/login',
		element: <Login />,
		action: LoginAction,
	},
	{
		path: '/signup',
		element: <Signup />,
		action: SignupAction,
	},
]);
ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
	<React.StrictMode>
		<RouterProvider router={router} />
	</React.StrictMode>,
)
