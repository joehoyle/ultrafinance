import React from 'react';
import { Form, Link, Params, redirect, useLoaderData } from "react-router-dom";
import { Function } from "../../../bindings/Function";
import { deleteFunction, getFunction, testFunction, updateFunction } from "../api";
import Button from "../components/Button";
import PageHeader from "../components/PageHeader";
import Editor from "@monaco-editor/react";
import * as monaco from 'monaco-editor/esm/vs/editor/editor.api';
import { UpdateFunction } from "../../../bindings/UpdateFunction";
import { FormEvent, useRef, useState } from "react";
import Input from "../components/forms/Input";
import FunctionParamField from '../components/FunctionParamField';
import FormDataToJson from '../utils/FormDataToJson';
import ErrorMessage from '../components/ErrorMessage';
import Textarea from '../components/forms/Textarea';
import { TestFunction } from '../../../bindings/TestFunction';
import { TestFunctionResult } from '../../../bindings/TestFunctionResult';

type RouteParams = Params;

export async function action({ request, params }: { request: Request, params: RouteParams }) {
	switch (request.method) {
		case 'POST':
			const formData = await request.formData();
			return updateFunction(Number(params.id), Object.fromEntries(formData.entries()) as unknown as UpdateFunction);
		case 'DELETE': {
			await deleteFunction(Number(params.id));
			return redirect('/functions');
		}
	}
}

export async function loader({ params }: { params: RouteParams }) {
	return {
		_function: await getFunction(Number(params.id!))
	}
}

export default function FunctionSingle() {
	const { _function } = useLoaderData() as { _function: Function };
	const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
	const sourceRef = useRef<HTMLInputElement>(null);
	const [isShowingTestRun, setIsShowingTestRun] = useState(false);
	const [isRunningTest, setIsRunningTest] = useState(false);
	const [testRunResult, setTestRunResult] = useState<TestFunctionResult | Error | undefined>();
	const [editorFullScreen, setEditorFullScreen] = useState(false);


	function onClickEditorFullScreen(e: React.MouseEvent<HTMLButtonElement, MouseEvent>) {
		e.preventDefault();
		setEditorFullScreen(!editorFullScreen);
	}

	function onClickShowTestRun(e: React.MouseEvent<HTMLButtonElement, MouseEvent>) {
		e.preventDefault();
		setIsShowingTestRun(!isShowingTestRun);
	}

	async function onTestRun(e: React.FormEvent<HTMLFormElement>) {
		e.preventDefault();
		setIsRunningTest(true);
		const formData = new FormData(e.target as HTMLFormElement);
		const data = FormDataToJson<TestFunction>(formData);
		try {
			const result = await testFunction(_function.id, {
				params: JSON.stringify(data.params),
				payload: data.payload,
			});
			setTestRunResult(result);
		} catch (e) {
			setTestRunResult(e as Error);
		}
		setIsRunningTest(false);
	}

	function handleEditorDidMount(editor: monaco.editor.IStandaloneCodeEditor) {
		editorRef.current = editor;
	}
	function onSubmit(event: FormEvent) {
		if (sourceRef.current && editorRef.current) {
			sourceRef.current.value = editorRef.current.getValue();
		}
	}
	return <>
		<PageHeader><Link to="/accounts">Functions</Link> &rarr; {_function.name}</PageHeader>
		<main className="flex-grow p-4">
			<div className="flex relative">
				<Form className="flex flex-col flex-1" method="post" action={`/functions/${_function.id}`} onSubmit={onSubmit}>
					<label>
						<div className="text-xs text-gray-500 mb-2">Function Name</div>
						<Input name="name" type="text" defaultValue={_function.name} />
					</label>
					<div className={`flex-1 py-2 bg-[#1e1e1e] ${editorFullScreen ? 'fixed inset-0' : 'relative mt-4 max-w-[calc(100vw-330px)]'}`}>
						<Editor
							height="70vh"
							defaultLanguage="typescript"
							defaultValue={_function.source}
							theme="vs-dark"
							options={{
								minimap: {
									enabled: false,
								},
								automaticLayout: true,
							}}
							onMount={handleEditorDidMount}

						/>
						<button onClick={onClickEditorFullScreen} className={`z-20 absolute top-4 ${editorFullScreen && isShowingTestRun ? 'right-[330px]' : 'right-4'} w-8 h-8 p-1 rounded bg-white/10 text-gray-500 hover:bg-white/20`}>
							<svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={1.5} stroke="currentColor" className="w-6 h-6">
								<path strokeLinecap="round" strokeLinejoin="round" d="M3.75 3.75v4.5m0-4.5h4.5m-4.5 0L9 9M3.75 20.25v-4.5m0 4.5h4.5m-4.5 0L9 15M20.25 3.75h-4.5m4.5 0v4.5m0-4.5L15 9m5.25 11.25h-4.5m4.5 0v-4.5m0 4.5L15 15" />
							</svg>
						</button>
					</div>
					<input ref={sourceRef} type="hidden" name="source" defaultValue={_function.source} />
					<div className="flex space-x-2 mt-4">
						<Button>Update Function</Button>
						<Button varient="secondary" onClick={onClickShowTestRun}>{isShowingTestRun ? 'Close Test' : 'Test'}</Button>
					</div>
				</Form>
				{isShowingTestRun &&
					<div className={`p-4 w-72 ${editorFullScreen ? 'fixed z-10 top-0 right-0 bottom-0' : 'absolute right-0 top-20 bottom-12'} bg-[#2e2e2e] flex flex-col dark`}>
						<h4 className="text-gray-300">Test Run</h4>

						<form className="flex flex-1 flex-col space-y-4 items-start max-h-[95%]" onSubmit={onTestRun}>
							<div className="flex flex-col items-stretch w-full space-y-2">
								{_function.params &&
									Object.entries(_function.params).map(([paramId, param]) => (
										<FunctionParamField key={paramId} id={paramId} param={param} value="" />
									))
								}
								<Textarea className="font-mono" rows={5} name="payload" placeholder="Enter JSON test data for event">{`{
    "id": 1062,
    "externalId": "CB20230106H3223006B2335956000003749",
    "creditorName": "easybell GmbH",
    "debtorName": null,
    "remittanceInformation": "easybell Rechnung vom 02.01.2023 Kundennummer 3845227",
    "bookingDate": "2023-01-10",
    "bookingDatetime": null,
    "transactionAmount": "-63.94",
    "transactionAmountCurrency": "EUR",
    "proprietaryBankTransactionCode": "DIRECT_DEBIT",
    "currencyExchangeRate": null,
    "currencyExchangeSourceCurrency": null,
    "currencyExchangeTargetCurrency": null,
    "accountId": 2,
    "createdAt": "2023-01-10T14:43:53",
    "updatedAt": "2023-01-10T14:43:53"
}`}</Textarea>
							</div>

							<Button isLoading={isRunningTest} disabled={isRunningTest}>Run</Button>

							{testRunResult && <div className="text-xs flex-1 overflow-auto">
								{testRunResult instanceof Error ? (
									<ErrorMessage className="text-xs">{testRunResult.message}</ErrorMessage>
								) : (
									<>
										<p className="text-gray-600">Result</p>
										<span className="dark:text-gray-300 font-mono whitespace-pre break-all"><JsonValue value={testRunResult.result} /></span>

										<p className="text-gray-600">Console</p>
										<span className="">
											{testRunResult.console.map((line, i) => (
												<span key={i} className="block dark:text-gray-300 font-mono whitespace-pre break-all"><JsonValue value={line.msg} /></span>
											))}
										</span>
									</>
								)}
							</div>}
						</form>
					</div>
				}
			</div>
			<Form method="delete" action={`/functions/${_function.id}`} className="mt-4">
				<Button varient="danger">Delete Function</Button>
			</Form>
		</main>
	</>
}

function JsonValue({ value }: { value: string }): string {
	try {
		return JSON.stringify(JSON.parse(value), null, 4);
	} catch (e) {
		return value;
	}
}
