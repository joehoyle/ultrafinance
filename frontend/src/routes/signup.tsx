import { Form, isRouteErrorResponse, Link, redirect, useActionData, useRouteError } from "react-router-dom";
import { CreateSession } from "../../../bindings/CreateSession";
import { NewUser } from "../../../bindings/NewUser";
import { createSession, createUser } from "../api";
import Button from "../components/Button";
import ErrorMessage from "../components/ErrorMessage";
import Input from "../components/forms/Input";

export async function action({ request }: { request: Request }) {
	const formData = await request.formData();
	try {
		await createUser(Object.fromEntries(formData.entries()) as unknown as NewUser);
		await createSession(Object.fromEntries(formData.entries()) as unknown as CreateSession);
	} catch (e) {
		return e;
	}
	return redirect('/accounts');
}

export default function Signup() {
	const error = useActionData();

	return <div className="min-h-screen flex items-center">
		<Form className="w-64 mx-auto flex flex-col space-y-4" method="post">
			<Link to="/"><img src={`${import.meta.env.BASE_URL}logo.svg`} width="80" className="mx-auto mb-4" /></Link>
			{error instanceof Error && <ErrorMessage className="w-full">
				<p className="max-w-sm">
					{error.message}
				</p>
			</ErrorMessage>}
			<label className="text-xs text-gray-400 flex flex-col">
				Name
				<Input name="name" type="text" />
			</label>
			<label className="text-xs text-gray-400 flex flex-col">
				Email Address
				<Input name="email" type="email" />
			</label>
			<label className="text-xs text-gray-400 flex flex-col">
				Password
				<Input name="password" type="password" />
			</label>
			<div className="flex space-x-2 items-center">
				<Button>Sign Up</Button>
				<Link to="/login" className="text-xs font-bold text-purple-500">or Log In</Link>
			</div>
		</Form>
	</div>
}
